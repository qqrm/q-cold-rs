use std::collections::HashMap;
use std::io::{self, Stdout};
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
    Frame, Terminal,
};

use crate::webapp::{
    self, DashboardState, QueueAppendRequest, QueueRemoveRequest, QueueRunItemRequest,
    QueueRunRequest, TerminalPane, TerminalSendRequest,
};

const TABS: [Tab; 5] = [
    Tab::Queue,
    Tab::Tasks,
    Tab::Agents,
    Tab::Terminals,
    Tab::Status,
];
const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn run() -> Result<u8> {
    let mut terminal = setup_terminal()?;
    let result = panic::catch_unwind(AssertUnwindSafe(|| run_app(&mut terminal.terminal)));
    terminal.restore()?;
    match result {
        Ok(result) => result.map(|()| 0),
        Err(payload) => panic::resume_unwind(payload),
    }
}

fn setup_terminal() -> Result<TerminalSession> {
    enable_raw_mode().context("failed to enable terminal raw mode")?;
    let mut stdout = io::stdout();
    if let Err(err) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(err).context("failed to enter alternate screen");
    }
    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok(TerminalSession {
            terminal,
            restored: false,
        }),
        Err(err) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            Err(err).context("failed to initialize terminal")
        }
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl TerminalSession {
    fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        disable_raw_mode().context("failed to disable terminal raw mode")?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
            .context("failed to leave alternate screen")?;
        self.terminal
            .show_cursor()
            .context("failed to show terminal cursor")
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if !self.restored {
            let _ = disable_raw_mode();
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            let _ = self.terminal.show_cursor();
        }
    }
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let mut app = TuiApp::new();
    loop {
        terminal
            .draw(|frame| render(frame, &mut app))
            .context("failed to render TUI")?;
        if event::poll(Duration::from_millis(250)).context("failed to poll terminal events")? {
            let Event::Key(key) = event::read().context("failed to read terminal event")? else {
                continue;
            };
            if app.handle_key(key) {
                break;
            }
        }
        if app.last_refresh.elapsed() >= AUTO_REFRESH_INTERVAL {
            app.refresh_silent();
        }
    }
    Ok(())
}

struct TuiApp {
    state: DashboardState,
    tab: Tab,
    queue_selected: usize,
    task_selected: usize,
    agent_selected: usize,
    terminal_selected: usize,
    terminal_scroll_from_bottom: u16,
    input: Option<String>,
    message: String,
    last_refresh: Instant,
}

impl TuiApp {
    fn new() -> Self {
        Self {
            state: webapp::dashboard_state_for_tui(),
            tab: Tab::Queue,
            queue_selected: 0,
            task_selected: 0,
            agent_selected: 0,
            terminal_selected: 0,
            terminal_scroll_from_bottom: 0,
            input: None,
            message: "r refresh | tab switch | arrows select | : command | q quit".to_string(),
            last_refresh: Instant::now(),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if let Some(mut input) = self.input.take() {
            return self.handle_input_key(key, &mut input);
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Tab | KeyCode::Right => self.next_tab(),
            KeyCode::BackTab | KeyCode::Left => self.previous_tab(),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.scroll_terminal(-8),
            KeyCode::PageUp => self.scroll_terminal(8),
            KeyCode::Char(':' | 'i') => self.input = Some(String::new()),
            KeyCode::Char('s') => self.queue_stop_or_continue(),
            KeyCode::Char('x') => self.queue_remove_selected(),
            KeyCode::Char('c') => self.queue_clear(),
            _ => {}
        }
        false
    }

    fn handle_input_key(&mut self, key: KeyEvent, input: &mut String) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.input = None;
                self.message = "command canceled".to_string();
            }
            KeyCode::Enter => {
                let command = input.trim().to_string();
                self.input = None;
                self.execute_command(&command);
            }
            KeyCode::Backspace => {
                input.pop();
                self.input = Some(input.clone());
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.push(ch);
                self.input = Some(input.clone());
            }
            _ => self.input = Some(input.clone()),
        }
        false
    }

    fn execute_command(&mut self, input: &str) {
        match parse_tui_command(input) {
            TuiCommand::Empty => {}
            TuiCommand::Quit => self.message = "press q to quit".to_string(),
            TuiCommand::Refresh => self.refresh(),
            TuiCommand::Stop => self.queue_stop(),
            TuiCommand::Continue => self.queue_continue(),
            TuiCommand::Clear => self.queue_clear(),
            TuiCommand::Remove => self.queue_remove_selected(),
            TuiCommand::Send(text) => self.send_terminal_text(&text),
            TuiCommand::Run(prompt) => self.queue_run_prompt(&prompt),
            TuiCommand::Append(prompt) => self.queue_append_prompt(&prompt),
        }
    }

    fn refresh(&mut self) {
        self.refresh_silent();
        self.message = format!("snapshot refreshed at {}", self.state.generated_at_unix);
    }

    fn refresh_silent(&mut self) {
        self.state = webapp::dashboard_state_for_tui();
        self.clamp_selection();
        self.last_refresh = Instant::now();
    }

    fn next_tab(&mut self) {
        self.tab = TABS[(self.tab.index() + 1) % TABS.len()];
    }

    fn previous_tab(&mut self) {
        let index = self.tab.index();
        self.tab = TABS[(index + TABS.len() - 1) % TABS.len()];
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.current_len();
        if len == 0 {
            return;
        }
        let selected = self.current_selection();
        let next = selected.saturating_add_signed(delta).min(len - 1);
        self.set_current_selection(next);
        if self.tab == Tab::Terminals {
            self.terminal_scroll_from_bottom = 0;
        }
    }

    fn scroll_terminal(&mut self, delta_from_bottom: i16) {
        if self.tab != Tab::Terminals {
            return;
        }
        if delta_from_bottom.is_negative() {
            self.terminal_scroll_from_bottom = self
                .terminal_scroll_from_bottom
                .saturating_sub(delta_from_bottom.unsigned_abs());
        } else {
            self.terminal_scroll_from_bottom = self
                .terminal_scroll_from_bottom
                .saturating_add(delta_from_bottom.unsigned_abs());
        }
    }

    fn current_len(&self) -> usize {
        match self.tab {
            Tab::Queue => self.state.queue.records.len(),
            Tab::Tasks => self.state.task_records.records.len(),
            Tab::Agents => self.state.agents.text.lines().count(),
            Tab::Terminals => self.state.terminals.records.len(),
            Tab::Status => 1,
        }
    }

    fn current_selection(&self) -> usize {
        match self.tab {
            Tab::Queue => self.queue_selected,
            Tab::Tasks => self.task_selected,
            Tab::Agents => self.agent_selected,
            Tab::Terminals => self.terminal_selected,
            Tab::Status => 0,
        }
    }

    fn set_current_selection(&mut self, selected: usize) {
        match self.tab {
            Tab::Queue => self.queue_selected = selected,
            Tab::Tasks => self.task_selected = selected,
            Tab::Agents => self.agent_selected = selected,
            Tab::Terminals => self.terminal_selected = selected,
            Tab::Status => {}
        }
    }

    fn clamp_selection(&mut self) {
        self.queue_selected = clamp_index(self.queue_selected, self.state.queue.records.len());
        self.task_selected = clamp_index(self.task_selected, self.state.task_records.records.len());
        self.terminal_selected =
            clamp_index(self.terminal_selected, self.state.terminals.records.len());
    }

    fn selected_terminal(&self) -> Option<&TerminalPane> {
        self.state.terminals.records.get(self.terminal_selected)
    }

    fn queue_stop_or_continue(&mut self) {
        if self.state.queue.run.as_ref().is_some_and(|run| run.status == "stopped") {
            self.queue_continue();
        } else {
            self.queue_stop();
        }
    }

    fn queue_stop(&mut self) {
        let response = webapp::queue_stop_for_tui();
        self.finish_mutation(response.ok, response.output);
    }

    fn queue_continue(&mut self) {
        let Some(run_id) = self.state.queue.run.as_ref().map(|run| run.id.clone()) else {
            self.message = "no queue run to continue".to_string();
            return;
        };
        let response = webapp::queue_continue_for_tui(run_id);
        self.finish_mutation(response.ok, response.output);
    }

    fn queue_clear(&mut self) {
        let run_id = self.state.queue.run.as_ref().map(|run| run.id.clone());
        let response = webapp::queue_clear_for_tui(run_id);
        self.finish_mutation(response.ok, response.output);
    }

    fn queue_remove_selected(&mut self) {
        let Some(run) = &self.state.queue.run else {
            self.message = "no queue run selected".to_string();
            return;
        };
        let Some(item) = self.state.queue.records.get(self.queue_selected) else {
            self.message = "no queue row selected".to_string();
            return;
        };
        let request = QueueRemoveRequest {
            run_id: run.id.clone(),
            item_id: item.id.clone(),
            task_id: Some(format!("task/{}", item.slug)),
            agent_id: item.agent_id.clone(),
        };
        let response = webapp::queue_remove_for_tui(&request);
        self.finish_mutation(response.ok, response.output);
    }

    fn send_terminal_text(&mut self, text: &str) {
        let Some(target) = self.selected_terminal().map(|pane| pane.target.clone()) else {
            self.message = "no terminal selected".to_string();
            return;
        };
        let request = TerminalSendRequest {
            target,
            text: Some(text.to_string()),
            mode: None,
            key: None,
            submit: Some(true),
        };
        let response = webapp::terminal_send_for_tui(&request);
        self.finish_mutation(response.ok, response.output);
    }

    fn queue_run_prompt(&mut self, prompt: &str) {
        match self.queue_request_item(prompt, Vec::new()) {
            Ok(mut item) => {
                item.depends_on = Some(Vec::new());
                let response = webapp::queue_run_for_tui(QueueRunRequest {
                    run_id: None,
                    execution_mode: Some("sequence".to_string()),
                    selected_agent_command: item.agent_command.clone().unwrap_or_default(),
                    selected_repo_root: item.repo_root.clone(),
                    selected_repo_name: item.repo_name.clone(),
                    items: vec![item],
                });
                self.finish_mutation(response.ok, response.output);
            }
            Err(err) => self.message = format!("{err:#}"),
        }
    }

    fn queue_append_prompt(&mut self, prompt: &str) {
        let Some(run) = self.state.queue.run.as_ref() else {
            self.message = "no active queue run to append".to_string();
            return;
        };
        let dependencies = if run.execution_mode == "graph" {
            graph_append_dependencies(&self.state.queue.records)
        } else {
            Vec::new()
        };
        match self.queue_request_item(prompt, dependencies) {
            Ok(mut item) => {
                if run.execution_mode != "graph" {
                    item.depends_on = Some(Vec::new());
                }
                let response = webapp::queue_append_for_tui(QueueAppendRequest {
                    run_id: run.id.clone(),
                    items: vec![item],
                });
                self.finish_mutation(response.ok, response.output);
            }
            Err(err) => self.message = format!("{err:#}"),
        }
    }

    fn queue_request_item(
        &self,
        prompt: &str,
        depends_on: Vec<String>,
    ) -> Result<QueueRunItemRequest> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("prompt is empty");
        }
        let command = self
            .state
            .available_agents
            .records
            .first()
            .map(|agent| agent.command.clone())
            .context("no available agent command")?;
        Ok(QueueRunItemRequest {
            id: None,
            prompt: prompt.to_string(),
            slug: None,
            depends_on: Some(depends_on),
            repo_root: Some(self.state.repository.root.clone()),
            repo_name: Some(self.state.repository.name.clone()),
            agent_command: Some(command),
        })
    }

    fn finish_mutation(&mut self, ok: bool, output: String) {
        self.state = webapp::dashboard_state_for_tui();
        self.clamp_selection();
        self.last_refresh = Instant::now();
        self.message = if ok { output } else { format!("error: {output}") };
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Queue,
    Tasks,
    Agents,
    Terminals,
    Status,
}

impl Tab {
    const fn label(self) -> &'static str {
        match self {
            Self::Queue => "Queue",
            Self::Tasks => "Tasks",
            Self::Agents => "Agents",
            Self::Terminals => "Terminals",
            Self::Status => "Status",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Queue => 0,
            Self::Tasks => 1,
            Self::Agents => 2,
            Self::Terminals => 3,
            Self::Status => 4,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TuiCommand {
    Empty,
    Refresh,
    Stop,
    Continue,
    Clear,
    Remove,
    Quit,
    Run(String),
    Append(String),
    Send(String),
}

pub(crate) fn parse_tui_command(input: &str) -> TuiCommand {
    let input = input.trim();
    if input.is_empty() {
        return TuiCommand::Empty;
    }
    let trimmed = input.strip_prefix(':').unwrap_or(input).trim();
    let (verb, rest) = trimmed
        .split_once(char::is_whitespace)
        .map_or((trimmed, ""), |(verb, rest)| (verb, rest.trim()));
    match verb {
        "r" | "refresh" => TuiCommand::Refresh,
        "q" | "quit" => TuiCommand::Quit,
        "stop" => TuiCommand::Stop,
        "continue" | "cont" => TuiCommand::Continue,
        "clear" => TuiCommand::Clear,
        "remove" | "rm" => TuiCommand::Remove,
        "run" => command_with_text(TuiCommand::Run, rest),
        "append" | "ap" => command_with_text(TuiCommand::Append, rest),
        "send" | "term" => command_with_text(TuiCommand::Send, rest),
        _ => TuiCommand::Send(trimmed.to_string()),
    }
}

fn command_with_text(f: impl FnOnce(String) -> TuiCommand, text: &str) -> TuiCommand {
    if text.trim().is_empty() {
        TuiCommand::Empty
    } else {
        f(text.trim().to_string())
    }
}

fn graph_append_dependencies(items: &[crate::state::QueueItemRow]) -> Vec<String> {
    let by_id = items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    let mut depths = HashMap::new();
    for item in items {
        let depth = graph_item_depth(item.id.as_str(), &by_id, &mut depths, &mut Vec::new());
        depths.insert(item.id.as_str(), depth);
    }
    let max_depth = depths.values().copied().max().unwrap_or(0);
    if max_depth == 0 {
        return Vec::new();
    }
    items
        .iter()
        .filter(|item| depths.get(item.id.as_str()).copied() == Some(max_depth - 1))
        .map(|item| item.id.clone())
        .collect()
}

fn graph_item_depth<'a>(
    id: &'a str,
    by_id: &HashMap<&'a str, &'a crate::state::QueueItemRow>,
    depths: &mut HashMap<&'a str, usize>,
    stack: &mut Vec<&'a str>,
) -> usize {
    if let Some(depth) = depths.get(id) {
        return *depth;
    }
    if stack.contains(&id) {
        return 0;
    }
    let Some(item) = by_id.get(id) else {
        return 0;
    };
    stack.push(id);
    let depth = item
        .depends_on
        .iter()
        .map(|dependency| graph_item_depth(dependency.as_str(), by_id, depths, stack) + 1)
        .max()
        .unwrap_or(0);
    stack.pop();
    depths.insert(id, depth);
    depth
}

fn terminal_tail_scroll(output: &str, visible_height: u16, from_bottom: u16) -> u16 {
    let line_count = output.lines().count();
    let tail = line_count.saturating_sub(usize::from(visible_height));
    u16::try_from(tail)
        .unwrap_or(u16::MAX)
        .saturating_sub(from_bottom)
}

fn clamp_index(index: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        index.min(len - 1)
    }
}

fn render(frame: &mut Frame<'_>, app: &mut TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(frame.area());
    render_header(frame, app, chunks[0]);
    render_tabs(frame, app, chunks[1]);
    match app.tab {
        Tab::Queue => render_queue(frame, app, chunks[2]),
        Tab::Tasks => render_tasks(frame, app, chunks[2]),
        Tab::Agents => render_agents(frame, app, chunks[2]),
        Tab::Terminals => render_terminals(frame, app, chunks[2]),
        Tab::Status => render_status(frame, app, chunks[2]),
    }
    render_footer(frame, app, chunks[3]);
    if let Some(input) = &app.input {
        render_prompt(frame, input);
    }
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let queue_status = app
        .state
        .queue
        .run
        .as_ref()
        .map_or("no queue".to_string(), |run| format!("queue {} {}", run.id, run.status));
    let line = format!(
        "{} [{}] | tasks open={} closed={} | terminals={} | {}",
        app.state.repository.name,
        app.state.repository.branch,
        app.state.task_records.open,
        app.state.task_records.closed,
        app.state.terminals.count,
        queue_status,
    );
    frame.render_widget(Paragraph::new(line).block(Block::default().borders(Borders::ALL)), area);
}

fn render_tabs(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let titles = TABS
        .iter()
        .map(|tab| Line::from(Span::raw(tab.label())))
        .collect::<Vec<_>>();
    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(tabs, area);
}

fn render_queue(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let items = app
        .state
        .queue
        .records
        .iter()
        .map(|item| {
            let line = format!(
                "{} | {} | task/{} | {}",
                item.status,
                item.agent_command,
                item.slug,
                first_line(&item.prompt)
            );
            ListItem::new(line)
        })
        .collect::<Vec<_>>();
    render_list(frame, area, "Queue", items, app.queue_selected);
}

fn render_tasks(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let items = app
        .state
        .task_records
        .records
        .iter()
        .map(|task| {
            let label = task.agent_label.as_deref().unwrap_or("-");
            ListItem::new(format!("{} | {} | {} | {}", task.status, task.id, label, task.title))
        })
        .collect::<Vec<_>>();
    render_list(frame, area, "Tasks", items, app.task_selected);
}

fn render_agents(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let block = Block::default().title("Agents").borders(Borders::ALL);
    frame.render_widget(
        Paragraph::new(app.state.agents.text.clone())
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_terminals(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(area);
    let items = app
        .state
        .terminals
        .records
        .iter()
        .map(|pane| ListItem::new(format!("{} | {}", pane.label, pane.target)))
        .collect::<Vec<_>>();
    render_list(frame, chunks[0], "Terminals", items, app.terminal_selected);
    let output = app
        .selected_terminal()
        .map_or("no terminal selected", |pane| pane.output.as_str());
    let scroll = terminal_tail_scroll(
        output,
        chunks[1].height.saturating_sub(2),
        app.terminal_scroll_from_bottom,
    );
    let paragraph = Paragraph::new(output.to_string())
        .block(Block::default().title("Scrollback").borders(Borders::ALL))
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, chunks[1]);
}

fn render_status(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let text = format!(
        "{}\n\n{}\n\nDaemon cwd: {}\nActive repo: {}\nAgent start template:\n{}",
        app.state.status.text,
        app.state.agents.text,
        app.state.daemon_cwd,
        app.state.repository.root,
        app.state.commands.agent_start_template,
    );
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().title("Status").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let text = format!(
        "{} | :run <prompt> :append <prompt> :send <text> | s stop/continue x remove c clear",
        app.message,
    );
    frame.render_widget(Paragraph::new(text).block(Block::default().borders(Borders::ALL)), area);
}

fn render_prompt(frame: &mut Frame<'_>, input: &str) {
    let area = centered_rect(78, 5, frame.area());
    frame.render_widget(Clear, area);
    let text = format!(":{input}");
    let block = Block::default().title("Command").borders(Borders::ALL);
    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn render_list(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    items: Vec<ListItem<'_>>,
    selected: usize,
) {
    let list = List::new(items)
        .block(Block::default().title(title).borders(Borders::ALL))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(height),
            Constraint::Min(1),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= 96 {
        return line.to_string();
    }
    let mut value = line.chars().take(93).collect::<String>();
    value.push_str("...");
    value
}

#[cfg(test)]
mod tests {
    use super::{graph_append_dependencies, parse_tui_command, terminal_tail_scroll, TuiCommand};
    use crate::state;

    #[test]
    fn parses_queue_commands() {
        assert_eq!(parse_tui_command(":run fix it"), TuiCommand::Run("fix it".to_string()));
        assert_eq!(
            parse_tui_command("append more work"),
            TuiCommand::Append("more work".to_string())
        );
        assert_eq!(parse_tui_command("stop"), TuiCommand::Stop);
        assert_eq!(parse_tui_command("continue"), TuiCommand::Continue);
    }

    #[test]
    fn treats_unknown_command_as_terminal_send() {
        assert_eq!(
            parse_tui_command("hello terminal"),
            TuiCommand::Send("hello terminal".to_string())
        );
    }

    #[test]
    fn graph_append_depends_on_previous_wave() {
        let items = vec![
            queue_item("bootstrap", &[]),
            queue_item("fanout-a", &["bootstrap"]),
            queue_item("fanout-b", &["bootstrap"]),
            queue_item("tail", &["fanout-a", "fanout-b"]),
        ];

        assert_eq!(
            graph_append_dependencies(&items),
            vec!["fanout-a".to_string(), "fanout-b".to_string()]
        );
    }

    #[test]
    fn terminal_scroll_starts_at_tail() {
        let output = (0..20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(terminal_tail_scroll(&output, 5, 0), 15);
        assert_eq!(terminal_tail_scroll(&output, 5, 8), 7);
    }

    fn queue_item(id: &str, depends_on: &[&str]) -> state::QueueItemRow {
        state::QueueItemRow {
            id: id.to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: depends_on.iter().map(|value| (*value).to_string()).collect(),
            prompt: String::new(),
            slug: id.to_string(),
            repo_root: None,
            repo_name: None,
            agent_command: "c1".to_string(),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        }
    }
}
