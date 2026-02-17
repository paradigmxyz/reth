use crate::download::manifest::{ComponentSelection, SnapshotComponentType, SnapshotManifest};
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
    collections::BTreeMap,
    io,
    time::{Duration, Instant},
};

/// Result of the interactive component selector.
pub enum SelectionResult {
    /// User confirmed selections with per-component ranges.
    Selected(BTreeMap<SnapshotComponentType, ComponentSelection>),
    /// User cancelled.
    Cancelled,
}

/// Ordered presets for cycling through distance options on chunked components.
const DISTANCE_PRESETS: [ComponentSelection; 5] = [
    ComponentSelection::None,
    ComponentSelection::Distance(10_064),
    ComponentSelection::Distance(100_000),
    ComponentSelection::Distance(1_000_000),
    ComponentSelection::All,
];

/// Presets for non-chunked optional components (Indexes).
const SIMPLE_PRESETS: [ComponentSelection; 2] = [ComponentSelection::None, ComponentSelection::All];

struct SelectorApp {
    manifest: SnapshotManifest,
    /// Which component types are available in the manifest.
    available: Vec<SnapshotComponentType>,
    /// Current selection for each available component.
    selections: Vec<ComponentSelection>,
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

        // Default to minimal: required=All, txs+changesets=Distance(10064), rest=None
        let selections = available
            .iter()
            .map(|ty| {
                if ty.is_required() {
                    ComponentSelection::All
                } else if ty.is_minimal() {
                    ComponentSelection::Distance(10_064)
                } else {
                    ComponentSelection::None
                }
            })
            .collect();

        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self { manifest, available, selections, cursor: 0, list_state }
    }

    fn cycle_right(&mut self) {
        if let Some(ty) = self.available.get(self.cursor) {
            if ty.is_required() {
                return;
            }
            let presets = if ty.is_chunked() { &DISTANCE_PRESETS[..] } else { &SIMPLE_PRESETS[..] };
            let current = self.selections[self.cursor];
            let idx = presets.iter().position(|p| *p == current).unwrap_or(0);
            self.selections[self.cursor] = presets[(idx + 1) % presets.len()];
        }
    }

    fn cycle_left(&mut self) {
        if let Some(ty) = self.available.get(self.cursor) {
            if ty.is_required() {
                return;
            }
            let presets = if ty.is_chunked() { &DISTANCE_PRESETS[..] } else { &SIMPLE_PRESETS[..] };
            let current = self.selections[self.cursor];
            let idx = presets.iter().position(|p| *p == current).unwrap_or(0);
            self.selections[self.cursor] = presets[(idx + presets.len() - 1) % presets.len()];
        }
    }

    fn select_all(&mut self) {
        for (i, _) in self.available.iter().enumerate() {
            self.selections[i] = ComponentSelection::All;
        }
    }

    fn select_minimal(&mut self) {
        for (i, ty) in self.available.iter().enumerate() {
            if ty.is_required() {
                self.selections[i] = ComponentSelection::All;
            } else if ty.is_minimal() {
                self.selections[i] = ComponentSelection::Distance(10_064);
            } else {
                self.selections[i] = ComponentSelection::None;
            }
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

    fn selection_map(&self) -> BTreeMap<SnapshotComponentType, ComponentSelection> {
        self.available.iter().copied().zip(self.selections.iter().copied()).collect()
    }

    fn total_chunks(&self) -> u64 {
        self.available
            .iter()
            .zip(&self.selections)
            .map(|(ty, sel)| match sel {
                ComponentSelection::None => 0,
                ComponentSelection::All => self.manifest.chunks_for_distance(*ty, None),
                ComponentSelection::Distance(d) => self.manifest.chunks_for_distance(*ty, Some(*d)),
            })
            .sum()
    }
}

/// Runs the interactive component selector TUI.
///
/// Shows available snapshot components with range selectors, chunk counts, and a preview
/// of the pruning config that will be generated.
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

        if crossterm::event::poll(timeout)? &&
            let Event::Key(key) = event::read()? &&
            key.kind == event::KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    return Ok(SelectionResult::Cancelled);
                }
                KeyCode::Enter => {
                    return Ok(SelectionResult::Selected(app.selection_map()));
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => app.cycle_right(),
                KeyCode::Left | KeyCode::Char('h') => app.cycle_left(),
                KeyCode::Char('a') => app.select_all(),
                KeyCode::Char('m') => app.select_minimal(),
                KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

fn format_selection(sel: &ComponentSelection) -> String {
    match sel {
        ComponentSelection::All => "All".to_string(),
        ComponentSelection::Distance(d) => format!("Last {d}"),
        ComponentSelection::None => "None".to_string(),
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
            let sel = &app.selections[i];
            let sel_str = format_selection(sel);

            let num_chunks = match sel {
                ComponentSelection::None => 0,
                ComponentSelection::All => app.manifest.chunks_for_distance(*ty, None),
                ComponentSelection::Distance(d) => app.manifest.chunks_for_distance(*ty, Some(*d)),
            };

            let chunks_str = if num_chunks > 0 && ty.is_chunked() {
                format!("{num_chunks} chunks")
            } else {
                String::new()
            };

            let required = if ty.is_required() { " (required)" } else { "" };

            let arrows = if ty.is_required() {
                "   "
            } else if matches!(sel, ComponentSelection::All) {
                "◂  "
            } else if matches!(sel, ComponentSelection::None) {
                "  ▸"
            } else {
                "◂ ▸"
            };

            let style = if ty.is_required() {
                Style::default().fg(Color::DarkGray)
            } else if matches!(sel, ComponentSelection::None) {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Green)
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {:<30}", ty.display_name()), style),
                Span::styled(
                    format!("{arrows} {:<12}", sel_str),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:>12}", chunks_str), style.add_modifier(Modifier::DIM)),
                Span::styled(required.to_string(), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let total_chunks = app.total_chunks();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Components — {total_chunks} archives")),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD).bg(Color::DarkGray))
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, chunks[1], &mut app.list_state);

    // Config preview — use describe_prune_config_from_selections
    let config_lines =
        crate::download::config_gen::describe_prune_config_from_selections(&app.selection_map());
    let config_text = config_lines.join("\n");
    let config_preview = Paragraph::new(config_text)
        .block(Block::default().borders(Borders::ALL).title("Generated reth.toml preview"))
        .style(Style::default().fg(Color::Yellow))
        .wrap(Wrap { trim: false });
    f.render_widget(config_preview, chunks[2]);

    // Footer
    let footer =
        Paragraph::new(" [←/→] adjust  [m] minimal  [a] all  [Enter] confirm  [Esc] cancel")
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[3]);
}
