use crate::download::{
    config_gen::describe_prune_config,
    manifest::{SnapshotComponentType, SnapshotManifest},
    DownloadProgress,
};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{
    io,
    time::{Duration, Instant},
};

/// Result of the interactive component selector.
pub enum SelectionResult {
    /// User confirmed a selection of components.
    Selected(Vec<SnapshotComponentType>),
    /// User cancelled.
    Cancelled,
}

struct SelectorApp {
    manifest: SnapshotManifest,
    /// Which component types are available in the manifest.
    available: Vec<SnapshotComponentType>,
    /// Whether each available component is selected.
    selected: Vec<bool>,
    /// Current cursor position.
    cursor: usize,
    /// List state for ratatui.
    list_state: ListState,
}

impl SelectorApp {
    fn new(manifest: SnapshotManifest) -> Self {
        let available: Vec<SnapshotComponentType> = SnapshotComponentType::ALL
            .iter()
            .copied()
            .filter(|ty| manifest.component(*ty).is_some())
            .collect();

        // State is always selected by default
        let selected = available.iter().map(|ty| ty.is_required()).collect();

        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self { manifest, available, selected, cursor: 0, list_state }
    }

    fn toggle_current(&mut self) {
        if let Some(ty) = self.available.get(self.cursor) {
            if !ty.is_required() {
                self.selected[self.cursor] = !self.selected[self.cursor];
            }
        }
    }

    fn select_all(&mut self) {
        for (i, _) in self.available.iter().enumerate() {
            self.selected[i] = true;
        }
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        } else {
            self.cursor = self.available.len().saturating_sub(1);
        }
        self.list_state.select(Some(self.cursor));
    }

    fn move_down(&mut self) {
        if self.cursor < self.available.len() - 1 {
            self.cursor += 1;
        } else {
            self.cursor = 0;
        }
        self.list_state.select(Some(self.cursor));
    }

    fn selected_types(&self) -> Vec<SnapshotComponentType> {
        self.available
            .iter()
            .zip(&self.selected)
            .filter(|(_, sel)| **sel)
            .map(|(ty, _)| *ty)
            .collect()
    }

    fn total_selected_size(&self) -> u64 {
        self.manifest.total_size(&self.selected_types())
    }
}

/// Runs the interactive component selector TUI.
///
/// Shows a checkbox list of available snapshot components with sizes, a running total, and a
/// preview of the pruning config that will be generated.
pub fn run_selector(manifest: SnapshotManifest) -> eyre::Result<SelectionResult> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = SelectorApp::new(manifest);
    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut SelectorApp,
) -> eyre::Result<SelectionResult> {
    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| render(f, app))?;

        let timeout =
            tick_rate.checked_sub(last_tick.elapsed()).unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            return Ok(SelectionResult::Cancelled);
                        }
                        KeyCode::Enter => {
                            return Ok(SelectionResult::Selected(app.selected_types()));
                        }
                        KeyCode::Char(' ') => app.toggle_current(),
                        KeyCode::Char('a') => app.select_all(),
                        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

fn render(f: &mut Frame<'_>, app: &mut SelectorApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Component list
            Constraint::Length(8), // Config preview
            Constraint::Length(3), // Footer
        ])
        .split(f.area());

    // Header
    let block_info = if app.manifest.block > 0 {
        format!(" (block {})", app.manifest.block)
    } else {
        String::new()
    };
    let header = Paragraph::new(format!(" Select snapshot components to download{}", block_info))
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL).title("reth download"));
    f.render_widget(header, chunks[0]);

    // Component list
    let items: Vec<ListItem<'_>> = app
        .available
        .iter()
        .enumerate()
        .map(|(i, ty)| {
            let checkbox = if app.selected[i] { "[x]" } else { "[ ]" };
            let size = app
                .manifest
                .component(*ty)
                .map(|c| DownloadProgress::format_size(c.total_size()))
                .unwrap_or_default();
            let required = if ty.is_required() { " (required)" } else { "" };

            let style = if ty.is_required() {
                Style::default().fg(Color::DarkGray)
            } else if app.selected[i] {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {checkbox} "), style),
                Span::styled(format!("{:<30}", ty.display_name()), style),
                Span::styled(format!("{:>10}", size), style.add_modifier(Modifier::DIM)),
                Span::styled(required.to_string(), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let total_str = DownloadProgress::format_size(app.total_selected_size());
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Components — Total: {total_str}")),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD).bg(Color::DarkGray))
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, chunks[1], &mut app.list_state);

    // Config preview
    let config_lines = describe_prune_config(&app.selected_types());
    let config_text = config_lines.join("\n");
    let config_preview = Paragraph::new(config_text)
        .block(Block::default().borders(Borders::ALL).title("Generated reth.toml preview"))
        .style(Style::default().fg(Color::Yellow))
        .wrap(Wrap { trim: false });
    f.render_widget(config_preview, chunks[2]);

    // Footer
    let footer = Paragraph::new(" [Space] toggle  [a] select all  [Enter] confirm  [Esc] cancel")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[3]);
}
