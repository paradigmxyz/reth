use crate::download::{
    manifest::{ComponentSelection, SnapshotComponentType, SnapshotManifest},
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
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
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
const DISTANCE_PRESETS: [ComponentSelection; 6] = [
    ComponentSelection::None,
    ComponentSelection::Distance(64),
    ComponentSelection::Distance(10_064),
    ComponentSelection::Distance(100_000),
    ComponentSelection::Distance(1_000_000),
    ComponentSelection::All,
];

/// A display group bundles one or more component types into a single TUI row.
struct DisplayGroup {
    /// Display name shown in the TUI.
    name: &'static str,
    /// Underlying component types this group controls.
    types: Vec<SnapshotComponentType>,
    /// Whether this group is required and locked to All.
    required: bool,
}

/// Build the display groups from available components in the manifest.
fn build_groups(manifest: &SnapshotManifest) -> Vec<DisplayGroup> {
    let has = |ty: SnapshotComponentType| manifest.component(ty).is_some();

    let mut groups = Vec::new();

    if has(SnapshotComponentType::State) {
        groups.push(DisplayGroup {
            name: "State (mdbx)",
            types: vec![SnapshotComponentType::State],
            required: true,
        });
    }

    if has(SnapshotComponentType::Headers) {
        groups.push(DisplayGroup {
            name: "Headers",
            types: vec![SnapshotComponentType::Headers],
            required: true,
        });
    }

    if has(SnapshotComponentType::Transactions) {
        groups.push(DisplayGroup {
            name: "Transactions",
            types: vec![SnapshotComponentType::Transactions],
            required: false,
        });
    }

    if has(SnapshotComponentType::Receipts) {
        groups.push(DisplayGroup {
            name: "Receipts",
            types: vec![SnapshotComponentType::Receipts],
            required: false,
        });
    }

    // Bundle account + storage changesets as "State History"
    let has_acc = has(SnapshotComponentType::AccountChangesets);
    let has_stor = has(SnapshotComponentType::StorageChangesets);
    if has_acc || has_stor {
        let mut types = Vec::new();
        if has_acc {
            types.push(SnapshotComponentType::AccountChangesets);
        }
        if has_stor {
            types.push(SnapshotComponentType::StorageChangesets);
        }
        groups.push(DisplayGroup { name: "State History", types, required: false });
    }

    groups
}

struct SelectorApp {
    manifest: SnapshotManifest,
    /// Display groups shown in the TUI.
    groups: Vec<DisplayGroup>,
    /// Current selection for each group.
    selections: Vec<ComponentSelection>,
    /// Current cursor position.
    cursor: usize,
    /// List state for ratatui.
    list_state: ListState,
}

impl SelectorApp {
    fn new(manifest: SnapshotManifest) -> Self {
        let groups = build_groups(&manifest);

        // Default to the minimal preset (matches --minimal prune config)
        let selections = groups.iter().map(|g| g.types[0].minimal_selection()).collect();

        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self { manifest, groups, selections, cursor: 0, list_state }
    }

    fn cycle_right(&mut self) {
        if let Some(group) = self.groups.get(self.cursor) {
            if group.required {
                return;
            }
            let current = self.selections[self.cursor];
            let idx = DISTANCE_PRESETS.iter().position(|p| *p == current).unwrap_or(0);
            self.selections[self.cursor] = DISTANCE_PRESETS[(idx + 1) % DISTANCE_PRESETS.len()];
        }
    }

    fn cycle_left(&mut self) {
        if let Some(group) = self.groups.get(self.cursor) {
            if group.required {
                return;
            }
            let current = self.selections[self.cursor];
            let idx = DISTANCE_PRESETS.iter().position(|p| *p == current).unwrap_or(0);
            self.selections[self.cursor] =
                DISTANCE_PRESETS[(idx + DISTANCE_PRESETS.len() - 1) % DISTANCE_PRESETS.len()];
        }
    }

    fn select_all(&mut self) {
        for sel in &mut self.selections {
            *sel = ComponentSelection::All;
        }
    }

    fn select_minimal(&mut self) {
        for (i, group) in self.groups.iter().enumerate() {
            self.selections[i] = group.types[0].minimal_selection();
        }
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        } else {
            self.cursor = self.groups.len().saturating_sub(1);
        }
        self.list_state.select(Some(self.cursor));
    }

    fn move_down(&mut self) {
        if self.cursor < self.groups.len() - 1 {
            self.cursor += 1;
        } else {
            self.cursor = 0;
        }
        self.list_state.select(Some(self.cursor));
    }

    /// Build the flat component→selection map from grouped selections.
    fn selection_map(&self) -> BTreeMap<SnapshotComponentType, ComponentSelection> {
        let mut map = BTreeMap::new();
        for (group, sel) in self.groups.iter().zip(&self.selections) {
            for ty in &group.types {
                map.insert(*ty, *sel);
            }
        }
        map
    }

    /// Size for a single group, summing all component types in the group.
    fn group_size(&self, group_idx: usize) -> u64 {
        let sel = self.selections[group_idx];
        let distance = match sel {
            ComponentSelection::None => return 0,
            ComponentSelection::All => None,
            ComponentSelection::Distance(d) => Some(d),
        };
        self.groups[group_idx]
            .types
            .iter()
            .map(|ty| self.manifest.size_for_distance(*ty, distance))
            .sum()
    }

    fn total_selected_size(&self) -> u64 {
        (0..self.groups.len()).map(|i| self.group_size(i)).sum()
    }
}

/// Runs the interactive component selector TUI.
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
            Constraint::Min(8),    // Component list
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
        .groups
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let sel = &app.selections[i];
            let sel_str = format_selection(sel);

            let size = app.group_size(i);
            let size_str =
                if size > 0 { DownloadProgress::format_size(size) } else { String::new() };

            let required = if group.required { " (required)" } else { "" };

            let arrows = if group.required {
                "   "
            } else if matches!(sel, ComponentSelection::All) {
                "◂  "
            } else if matches!(sel, ComponentSelection::None) {
                "  ▸"
            } else {
                "◂ ▸"
            };

            let style = if group.required {
                Style::default().fg(Color::DarkGray)
            } else if matches!(sel, ComponentSelection::None) {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Green)
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {:<22}", group.name), style),
                Span::styled(
                    format!("{arrows} {:<12}", sel_str),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:>10}", size_str), style.add_modifier(Modifier::DIM)),
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

    // Footer
    let footer =
        Paragraph::new(" [←/→] adjust  [m] minimal  [a] all  [Enter] confirm  [Esc] cancel")
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}
