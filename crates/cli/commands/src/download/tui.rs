use crate::download::{
    manifest::{ComponentSelection, SnapshotComponentType, SnapshotManifest},
    DownloadProgress, SelectionPreset,
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
pub struct SelectorOutput {
    /// User-confirmed selections with per-component ranges.
    pub selections: BTreeMap<SnapshotComponentType, ComponentSelection>,
    /// Last preset action used in the TUI, if any.
    pub preset: Option<SelectionPreset>,
}

/// All distance presets. Groups filter this to only valid options.
const DISTANCE_PRESETS: [ComponentSelection; 6] = [
    ComponentSelection::None,
    ComponentSelection::Distance(64),
    ComponentSelection::Distance(10_064),
    ComponentSelection::Distance(100_000),
    ComponentSelection::Distance(1_000_000),
    ComponentSelection::All,
];

/// Presets for components that require at least 64 blocks (receipts).
const RECEIPTS_PRESETS: [ComponentSelection; 5] = [
    ComponentSelection::Distance(64),
    ComponentSelection::Distance(10_064),
    ComponentSelection::Distance(100_000),
    ComponentSelection::Distance(1_000_000),
    ComponentSelection::All,
];

/// Presets for components that require at least 10064 blocks (account/storage history).
const HISTORY_PRESETS: [ComponentSelection; 4] = [
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
    /// Valid presets for this group. Components with minimum distance requirements
    /// exclude presets that would produce invalid prune configs.
    presets: &'static [ComponentSelection],
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
            presets: &DISTANCE_PRESETS,
        });
    }

    if has(SnapshotComponentType::Headers) {
        groups.push(DisplayGroup {
            name: "Headers",
            types: vec![SnapshotComponentType::Headers],
            required: true,
            presets: &DISTANCE_PRESETS,
        });
    }

    if has(SnapshotComponentType::Transactions) {
        groups.push(DisplayGroup {
            name: "Transactions",
            types: vec![SnapshotComponentType::Transactions],
            required: false,
            presets: &HISTORY_PRESETS,
        });
    }

    if has(SnapshotComponentType::Receipts) {
        groups.push(DisplayGroup {
            name: "Receipts",
            types: vec![SnapshotComponentType::Receipts],
            required: false,
            presets: &RECEIPTS_PRESETS,
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
        groups.push(DisplayGroup {
            name: "State History",
            types,
            required: false,
            presets: &HISTORY_PRESETS,
        });
    }

    groups
}

struct SelectorApp {
    manifest: SnapshotManifest,
    full_preset: BTreeMap<SnapshotComponentType, ComponentSelection>,
    /// Display groups shown in the TUI.
    groups: Vec<DisplayGroup>,
    /// Current selection for each group.
    selections: Vec<ComponentSelection>,
    /// Last preset action invoked by user.
    preset: Option<SelectionPreset>,
    /// Current cursor position.
    cursor: usize,
    /// List state for ratatui.
    list_state: ListState,
}

impl SelectorApp {
    fn new(
        manifest: SnapshotManifest,
        full_preset: BTreeMap<SnapshotComponentType, ComponentSelection>,
    ) -> Self {
        let groups = build_groups(&manifest);

        // Default to the minimal preset (matches --minimal prune config)
        let selections = groups.iter().map(|g| g.types[0].minimal_selection()).collect();

        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self {
            manifest,
            full_preset,
            groups,
            selections,
            preset: Some(SelectionPreset::Minimal),
            cursor: 0,
            list_state,
        }
    }

    fn cycle_right(&mut self) {
        if let Some(group) = self.groups.get(self.cursor) {
            if group.required {
                return;
            }
            let presets = group.presets;
            let current = self.selections[self.cursor];
            let idx = presets.iter().position(|p| *p == current).unwrap_or(0);
            self.selections[self.cursor] = presets[(idx + 1) % presets.len()];
            self.preset = None;
        }
    }

    fn cycle_left(&mut self) {
        if let Some(group) = self.groups.get(self.cursor) {
            if group.required {
                return;
            }
            let presets = group.presets;
            let current = self.selections[self.cursor];
            let idx = presets.iter().position(|p| *p == current).unwrap_or(0);
            self.selections[self.cursor] = presets[(idx + presets.len() - 1) % presets.len()];
            self.preset = None;
        }
    }

    fn select_all(&mut self) {
        for sel in &mut self.selections {
            *sel = ComponentSelection::All;
        }
        self.preset = Some(SelectionPreset::Archive);
    }

    fn select_minimal(&mut self) {
        for (i, group) in self.groups.iter().enumerate() {
            self.selections[i] = group.types[0].minimal_selection();
        }
        self.preset = Some(SelectionPreset::Minimal);
    }

    fn select_full(&mut self) {
        for (i, group) in self.groups.iter().enumerate() {
            let mut selection = group.types[0].minimal_selection();
            for ty in &group.types {
                if let Some(sel) = self.full_preset.get(ty).copied() {
                    selection = sel;
                    break;
                }
            }
            self.selections[i] = selection;
        }
        self.preset = Some(SelectionPreset::Full);
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
            ComponentSelection::Since(block) => Some(self.manifest.block - block + 1),
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
pub fn run_selector(
    manifest: SnapshotManifest,
    full_preset: &BTreeMap<SnapshotComponentType, ComponentSelection>,
) -> eyre::Result<SelectorOutput> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = SelectorApp::new(manifest, full_preset.clone());
    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut SelectorApp,
) -> eyre::Result<SelectorOutput> {
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
                    eyre::bail!("Download cancelled by user");
                }
                KeyCode::Enter => {
                    return Ok(SelectorOutput {
                        selections: app.selection_map(),
                        preset: app.preset,
                    });
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => app.cycle_right(),
                KeyCode::Left | KeyCode::Char('h') => app.cycle_left(),
                KeyCode::Char('a') => app.select_all(),
                KeyCode::Char('f') => app.select_full(),
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
        ComponentSelection::Distance(d) => format!("Last {d} blocks"),
        ComponentSelection::Since(block) => format!("Since block {block}"),
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

            let at_max = *sel == *group.presets.last().unwrap_or(&ComponentSelection::All);
            let at_min = *sel == group.presets[0];
            let arrows = if group.required {
                "   "
            } else if at_max {
                "◂  "
            } else if at_min {
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
    let footer = Paragraph::new(
        " [←/→] adjust  [m] minimal  [f] full  [a] archive  [Enter] confirm  [Esc] cancel",
    )
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}
