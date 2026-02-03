use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChoice {
    Once,
    Session,
    Deny,
}

pub enum TuiEvent {
    Input(String),
    Permission(PermissionChoice),
    ModelPick(String),
    Quit,
    None,
}

pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    input: String,
    history: Vec<String>,
    output: Vec<OutputLine>,
    debug: Vec<String>,
    show_debug: bool,
    status: String,
    pending_permission: Option<(String, Vec<String>)>,
    pending_model_picker: Option<Vec<ModelChoice>>,
    busy: bool,
    current_model: Option<String>,
    spinner_index: usize,
    last_spinner_tick: Instant,
}

#[derive(Debug, Clone)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputKind {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
struct OutputLine {
    text: String,
    kind: OutputKind,
}

impl Tui {
    pub fn new() -> Result<Self, io::Error> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            input: String::new(),
            history: Vec::new(),
            output: vec![OutputLine {
                text: "PicoBot ready. Type /quit to exit.".to_string(),
                kind: OutputKind::System,
            }],
            debug: Vec::new(),
            show_debug: false,
            status: "F2: Debug  F3: Help  Ctrl+C: Quit".to_string(),
            pending_permission: None,
            pending_model_picker: None,
            busy: false,
            current_model: None,
            spinner_index: 0,
            last_spinner_tick: Instant::now(),
        })
    }

    pub fn next_event(&mut self) -> Result<TuiEvent, io::Error> {
        self.next_event_with_timeout(Duration::from_millis(50))
    }

    pub fn next_event_with_timeout(&mut self, timeout: Duration) -> Result<TuiEvent, io::Error> {
        self.draw()?;
        if !event::poll(timeout)? {
            return Ok(TuiEvent::None);
        }
        let Event::Key(key) = event::read()? else {
            return Ok(TuiEvent::None);
        };
        if key.kind != KeyEventKind::Press {
            return Ok(TuiEvent::None);
        }

        if self.pending_permission.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.pending_permission = None;
                    return Ok(TuiEvent::Permission(PermissionChoice::Once));
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    self.pending_permission = None;
                    return Ok(TuiEvent::Permission(PermissionChoice::Session));
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.pending_permission = None;
                    return Ok(TuiEvent::Permission(PermissionChoice::Deny));
                }
                _ => return Ok(TuiEvent::None),
            }
        }

        if let Some(models) = &self.pending_model_picker {
            match key.code {
                KeyCode::Esc => {
                    self.pending_model_picker = None;
                    return Ok(TuiEvent::None);
                }
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    let index = ch.to_digit(10).unwrap_or(0) as usize;
                    if index > 0 && index <= models.len() {
                        let selected = models[index - 1].id.clone();
                        self.pending_model_picker = None;
                        return Ok(TuiEvent::ModelPick(selected));
                    }
                }
                KeyCode::Enter => {
                    self.pending_model_picker = None;
                    return Ok(TuiEvent::None);
                }
                _ => return Ok(TuiEvent::None),
            }
        }

        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Ok(TuiEvent::Quit)
            }
            KeyCode::F(2) => {
                self.show_debug = !self.show_debug;
                Ok(TuiEvent::None)
            }
            KeyCode::F(3) => {
                self.push_system("Commands: /help /quit /exit /clear /permissions /models");
                Ok(TuiEvent::None)
            }
            KeyCode::Char(ch) => {
                self.input.push(ch);
                Ok(TuiEvent::None)
            }
            KeyCode::Backspace => {
                self.input.pop();
                Ok(TuiEvent::None)
            }
            KeyCode::Up => {
                if let Some(last) = self.history.last() {
                    self.input = last.clone();
                }
                Ok(TuiEvent::None)
            }
            KeyCode::Enter => {
                let line = self.input.trim().to_string();
                self.history.push(line.clone());
                self.input.clear();
                if line == "/quit" || line == "/exit" {
                    Ok(TuiEvent::Quit)
                } else {
                    Ok(TuiEvent::Input(line))
                }
            }
            _ => Ok(TuiEvent::None),
        }
    }

    pub fn push_output(&mut self, line: impl Into<String>) {
        self.push_system(line);
    }

    pub fn push_system(&mut self, line: impl Into<String>) {
        self.output.push(OutputLine {
            text: line.into(),
            kind: OutputKind::System,
        });
    }

    pub fn push_user(&mut self, line: impl Into<String>) {
        self.output.push(OutputLine {
            text: line.into(),
            kind: OutputKind::User,
        });
    }

    pub fn push_assistant(&mut self, line: impl Into<String>) {
        self.output.push(OutputLine {
            text: line.into(),
            kind: OutputKind::Assistant,
        });
    }

    pub fn append_output(&mut self, chunk: &str) {
        if self.output.is_empty() {
            self.push_assistant(String::new());
        }
        if let Some(last) = self.output.last()
            && last.kind != OutputKind::Assistant
        {
            self.push_assistant(String::new());
        }
        let mut parts = chunk.split('\n').peekable();
        if let Some(first) = parts.next()
            && let Some(last) = self.output.last_mut()
        {
            last.text.push_str(first);
        }
        for part in parts {
            self.push_assistant(part.to_string());
        }
    }

    pub fn clear_output(&mut self) {
        self.output.clear();
    }

    pub fn push_debug(&mut self, line: impl Into<String>) {
        self.debug.push(line.into());
    }

    pub fn drain_debug(&mut self) -> Vec<String> {
        self.debug.drain(..).collect()
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub fn set_pending_permission(&mut self, tool: impl Into<String>, permissions: Vec<String>) {
        self.pending_permission = Some((tool.into(), permissions));
    }

    pub fn clear_pending_permission(&mut self) {
        self.pending_permission = None;
    }

    pub fn has_pending_permission(&self) -> bool {
        self.pending_permission.is_some()
    }

    pub fn set_pending_model_picker(&mut self, models: Vec<ModelChoice>) {
        self.pending_model_picker = Some(models);
    }

    pub fn clear_pending_model_picker(&mut self) {
        self.pending_model_picker = None;
    }

    pub fn refresh(&mut self) -> Result<(), io::Error> {
        self.draw()
    }

    pub fn start_assistant_message(&mut self) {
        self.push_assistant(String::new());
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    pub fn set_current_model(&mut self, model: impl Into<String>) {
        self.current_model = Some(model.into());
    }

    fn draw(&mut self) -> Result<(), io::Error> {
        let output = output_to_text(&self.output);
        let debug = debug_to_text(&self.debug);
        let input = self.input.clone();
        let status = self.status.clone();
        let permission = self.pending_permission.clone();
        let model_picker = self.pending_model_picker.clone();
        let busy = self.busy;
        let model_label = self
            .current_model
            .clone()
            .unwrap_or_else(|| "(none)".to_string());
        if self.busy && self.last_spinner_tick.elapsed().as_millis() > 120 {
            self.spinner_index = (self.spinner_index + 1) % SPINNER_FRAMES.len();
            self.last_spinner_tick = Instant::now();
        }

        self.terminal.draw(|frame| {
            let area = frame.area();
            let base = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(3),
                    Constraint::Length(2),
                ])
                .split(area);

            if self.show_debug {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
                    .split(base[0]);
                let output_block = Block::default().title("PicoBot").borders(Borders::ALL);
                let output_widget = Paragraph::new(output.clone())
                    .block(output_block)
                    .wrap(Wrap { trim: false });
                frame.render_widget(output_widget, cols[0]);

                let debug_block = Block::default().title("Debug").borders(Borders::ALL);
                let debug_widget = Paragraph::new(debug.clone())
                    .block(debug_block)
                    .wrap(Wrap { trim: false });
                frame.render_widget(debug_widget, cols[1]);
            } else {
                let output_block = Block::default().title("PicoBot").borders(Borders::ALL);
                let output_widget = Paragraph::new(output.clone())
                    .block(output_block)
                    .wrap(Wrap { trim: false });
                frame.render_widget(output_widget, base[0]);
            }

            let input_block = Block::default().title("Input").borders(Borders::ALL);
            let input_widget = Paragraph::new(input).block(input_block);
            frame.render_widget(input_widget, base[1]);
            let cursor_x = base[1].x + 1 + self.input.len() as u16;
            let cursor_y = base[1].y + 1;
            frame.set_cursor_position((cursor_x, cursor_y));

            let status_widget = Paragraph::new(status_line(
                status,
                &model_label,
                busy,
                SPINNER_FRAMES[self.spinner_index],
            ));
            frame.render_widget(status_widget, base[2]);

            if let Some((tool, permissions)) = permission {
                let text = format!(
                    "Permission required for tool '{tool}': {}\nAllow once (Y) / session (S) / deny (N)",
                    permissions.join(", ")
                );
                let popup_area = centered_rect(70, 30, area);
                let block = Block::default().title("Permission").borders(Borders::ALL);
                let widget = Paragraph::new(text).block(block).wrap(Wrap { trim: true });
                frame.render_widget(widget, popup_area);
            }

            if let Some(models) = model_picker {
                let text = model_picker_text(&models);
                let popup_area = centered_rect(70, 40, area);
                let block = Block::default().title("Models").borders(Borders::ALL);
                let widget = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
                frame.render_widget(widget, popup_area);
            }
        })?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, rect: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(rect);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn output_to_text(lines: &[OutputLine]) -> Text<'_> {
    let mut rendered = Vec::new();
    for line in lines {
        let style = match line.kind {
            OutputKind::User => Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            OutputKind::Assistant => Style::default().fg(Color::White),
            OutputKind::System => Style::default().fg(Color::Yellow),
        };
        rendered.push(Line::from(Span::styled(line.text.clone(), style)));
    }
    Text::from(rendered)
}

fn debug_to_text(lines: &[String]) -> Text<'_> {
    let style = Style::default().fg(Color::Gray);
    let rendered = lines
        .iter()
        .map(|line| Line::from(Span::styled(line.clone(), style)))
        .collect::<Vec<_>>();
    Text::from(rendered)
}

const SPINNER_FRAMES: [&str; 4] = ["-", "\\", "|", "/"];

fn status_line<'a>(status: String, model_label: &'a str, busy: bool, spinner: &'a str) -> Text<'a> {
    let mut spans = Vec::new();
    spans.push(Span::styled(status, Style::default().fg(Color::Gray)));
    spans.push(Span::raw("  |  "));
    spans.push(Span::styled(
        format!("Model: {model_label}"),
        Style::default().fg(Color::LightBlue),
    ));
    spans.push(Span::raw("  |  "));
    spans.push(Span::styled(
        if busy {
            format!("{spinner} Busy")
        } else {
            "Idle".to_string()
        },
        if busy {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        },
    ));
    Text::from(Line::from(spans))
}

fn model_picker_text(models: &[ModelChoice]) -> Text<'_> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Select a model (number). Esc to cancel.",
        Style::default().fg(Color::Yellow),
    )));
    for (index, model) in models.iter().enumerate() {
        let label = format!(
            "{index_plus}. {label}",
            index_plus = index + 1,
            label = model.label
        );
        lines.push(Line::from(Span::raw(label)));
    }
    Text::from(lines)
}
