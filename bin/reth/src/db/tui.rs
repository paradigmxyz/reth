use crossterm::{
    event::{self, Event, KeyCode, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use reth_db::{
    table::{Table, TableRow},
    RawValue,
};
use std::{
    io,
    time::{Duration, Instant},
};
use tracing::error;
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Corner, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};

/// Available keybindings for the [DbListTUI]
static CMDS: [(&str, &str); 6] = [
    ("q", "Quit"),
    ("↑", "Entry above"),
    ("↓", "Entry below"),
    ("←", "Previous page"),
    ("→", "Next page"),
    ("G", "Go to a specific page"),
];

/// Modified version of the [ListState] struct that exposes the `offset` field.
/// Used to make the [DbListTUI] keys clickable.
struct ExpListState {
    pub(crate) offset: usize,
}

#[derive(Default, Eq, PartialEq)]
pub(crate) enum ViewMode {
    /// Normal list view mode
    #[default]
    Normal,
    /// Currently wanting to go to a page
    GoToPage,
}

enum Entries<T: Table> {
    /// Pairs of [Table::Key] and [RawValue<Table::Value>]
    RawValues(Vec<(T::Key, RawValue<T::Value>)>),
    /// Pairs of [Table::Key] and [Table::Value]
    Values(Vec<TableRow<T>>),
}

impl<T: Table> Entries<T> {
    /// Creates new empty [Entries] as [Entries::RawValues] if `raw_values == true` and as
    /// [Entries::Values] if `raw == false`.
    fn new_with_raw_values(raw_values: bool) -> Self {
        if raw_values {
            Self::RawValues(Vec::new())
        } else {
            Self::Values(Vec::new())
        }
    }

    /// Sets the internal entries [Vec], converting the [Table::Value] into [RawValue<Table::Value>]
    /// if needed.
    fn set(&mut self, new_entries: Vec<TableRow<T>>) {
        match self {
            Entries::RawValues(old_entries) => {
                *old_entries =
                    new_entries.into_iter().map(|(key, value)| (key, value.into())).collect()
            }
            Entries::Values(old_entries) => *old_entries = new_entries,
        }
    }

    /// Returns the length of internal [Vec].
    fn len(&self) -> usize {
        match self {
            Entries::RawValues(entries) => entries.len(),
            Entries::Values(entries) => entries.len(),
        }
    }

    /// Returns an iterator over keys of the internal [Vec]. For both [Entries::RawValues] and
    /// [Entries::Values], this iterator will yield [Table::Key].
    fn iter_keys(&self) -> EntriesKeyIter<'_, T> {
        EntriesKeyIter { entries: self, index: 0 }
    }
}

struct EntriesKeyIter<'a, T: Table> {
    entries: &'a Entries<T>,
    index: usize,
}

impl<'a, T: Table> Iterator for EntriesKeyIter<'a, T> {
    type Item = &'a T::Key;

    fn next(&mut self) -> Option<Self::Item> {
        let item = match self.entries {
            Entries::RawValues(values) => values.get(self.index).map(|(key, _)| key),
            Entries::Values(values) => values.get(self.index).map(|(key, _)| key),
        };
        self.index += 1;

        item
    }
}

pub(crate) struct DbListTUI<F, T: Table>
where
    F: FnMut(usize, usize) -> Vec<TableRow<T>>,
{
    /// Fetcher for the next page of items.
    ///
    /// The fetcher is passed the index of the first item to fetch, and the number of items to
    /// fetch from that item.
    fetch: F,
    /// Skip N indices of the key list in the DB.
    skip: usize,
    /// The amount of entries to show per page
    count: usize,
    /// The total number of entries in the database
    total_entries: usize,
    /// The current view mode
    mode: ViewMode,
    /// The current state of the input buffer
    input: String,
    /// The state of the key list.
    list_state: ListState,
    /// Entries to show in the TUI.
    entries: Entries<T>,
}

impl<F, T: Table> DbListTUI<F, T>
where
    F: FnMut(usize, usize) -> Vec<TableRow<T>>,
{
    /// Create a new database list TUI
    pub(crate) fn new(
        fetch: F,
        skip: usize,
        count: usize,
        total_entries: usize,
        raw: bool,
    ) -> Self {
        Self {
            fetch,
            skip,
            count,
            total_entries,
            mode: ViewMode::Normal,
            input: String::new(),
            list_state: ListState::default(),
            entries: Entries::new_with_raw_values(raw),
        }
    }

    /// Move to the next list selection
    fn next(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.entries.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    /// Move to the previous list selection
    fn previous(&mut self) {
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.entries.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn reset(&mut self) {
        self.list_state.select(Some(0));
    }

    /// Fetch the next page of items
    fn next_page(&mut self) {
        if self.skip + self.count >= self.total_entries {
            return
        }

        self.skip += self.count;
        self.fetch_page();
    }

    /// Fetch the previous page of items
    fn previous_page(&mut self) {
        if self.skip == 0 {
            return
        }

        self.skip = self.skip.saturating_sub(self.count);
        self.fetch_page();
    }

    /// Go to a specific page.
    fn go_to_page(&mut self, page: usize) {
        self.skip = (self.count * page).min(self.total_entries - self.count);
        self.fetch_page();
    }

    /// Fetch the current page
    fn fetch_page(&mut self) {
        self.entries.set((self.fetch)(self.skip, self.count));
        self.reset();
    }

    /// Show the [DbListTUI] in the terminal.
    pub(crate) fn run(mut self) -> eyre::Result<()> {
        // Setup backend
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Load initial page
        self.fetch_page();

        // Run event loop
        let tick_rate = Duration::from_millis(250);
        let res = event_loop(&mut terminal, &mut self, tick_rate);

        // Restore terminal
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        // Handle errors
        if let Err(err) = res {
            error!("{:?}", err)
        }
        Ok(())
    }
}

/// Run the event loop
fn event_loop<B: Backend, F, T: Table>(
    terminal: &mut Terminal<B>,
    app: &mut DbListTUI<F, T>,
    tick_rate: Duration,
) -> io::Result<()>
where
    F: FnMut(usize, usize) -> Vec<TableRow<T>>,
{
    let mut last_tick = Instant::now();
    let mut running = true;
    while running {
        // Render
        terminal.draw(|f| ui(f, app))?;

        // Calculate timeout
        let timeout =
            tick_rate.checked_sub(last_tick.elapsed()).unwrap_or_else(|| Duration::from_secs(0));

        // Poll events
        if crossterm::event::poll(timeout)? {
            running = !handle_event(app, event::read()?)?;
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    Ok(())
}

/// Handle incoming events
fn handle_event<F, T: Table>(app: &mut DbListTUI<F, T>, event: Event) -> io::Result<bool>
where
    F: FnMut(usize, usize) -> Vec<TableRow<T>>,
{
    if app.mode == ViewMode::GoToPage {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Enter => {
                    let input = std::mem::take(&mut app.input);
                    if let Ok(page) = input.parse() {
                        app.go_to_page(page);
                    }
                    app.mode = ViewMode::Normal;
                }
                KeyCode::Char(c) => {
                    app.input.push(c);
                }
                KeyCode::Backspace => {
                    app.input.pop();
                }
                KeyCode::Esc => app.mode = ViewMode::Normal,
                _ => {}
            }
        }

        return Ok(false)
    }

    match event {
        Event::Key(key) => match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(true),
            KeyCode::Down => app.next(),
            KeyCode::Up => app.previous(),
            KeyCode::Right => app.next_page(),
            KeyCode::Left => app.previous_page(),
            KeyCode::Char('G') => {
                app.mode = ViewMode::GoToPage;
            }
            _ => {}
        },
        Event::Mouse(e) => match e.kind {
            MouseEventKind::ScrollDown => app.next(),
            MouseEventKind::ScrollUp => app.previous(),
            // TODO: This click event can be triggered outside of the list widget.
            MouseEventKind::Down(_) => {
                // SAFETY: The pointer to the app's state will always be valid for
                // reads here, and the source is larger than the destination.
                //
                // This is technically unsafe, but because the alignment requirements
                // in both the source and destination are the same and we can ensure
                // that the pointer to `app.state` is valid for reads, this is safe.
                let state: ExpListState = unsafe { std::mem::transmute_copy(&app.list_state) };
                let new_idx = (e.row as usize + state.offset).saturating_sub(1);
                if new_idx < app.entries.len() {
                    app.list_state.select(Some(new_idx));
                }
            }
            _ => {}
        },
        _ => {}
    }

    Ok(false)
}

/// Render the UI
fn ui<B: Backend, F, T: Table>(f: &mut Frame<'_, B>, app: &mut DbListTUI<F, T>)
where
    F: FnMut(usize, usize) -> Vec<TableRow<T>>,
{
    let outer_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(95), Constraint::Percentage(5)].as_ref())
        .split(f.size());

    // Columns
    {
        let inner_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(outer_chunks[0]);

        let key_length = format!("{}", (app.skip + app.count).saturating_sub(1)).len();

        let formatted_keys = app
            .entries
            .iter_keys()
            .enumerate()
            .map(|(i, k)| {
                ListItem::new(format!("[{:0>width$}]: {k:?}", i + app.skip, width = key_length))
            })
            .collect::<Vec<ListItem<'_>>>();

        let key_list = List::new(formatted_keys)
            .block(Block::default().borders(Borders::ALL).title(format!(
                "Keys (Showing entries {}-{} out of {} entries)",
                app.skip,
                (app.skip + app.entries.len()).saturating_sub(1),
                app.total_entries
            )))
            .style(Style::default().fg(Color::White))
            .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::ITALIC))
            .highlight_symbol("➜ ")
            .start_corner(Corner::TopLeft);
        f.render_stateful_widget(key_list, inner_chunks[0], &mut app.list_state);

        let values: Vec<_> = match &app.entries {
            Entries::RawValues(entries) => entries
                .iter()
                .map(|(_, v)| {
                    serde_json::to_string_pretty(v)
                        .unwrap_or(String::from("Error serializing value"))
                })
                .collect(),
            Entries::Values(entries) => entries
                .iter()
                .map(|(_, v)| {
                    serde_json::to_string_pretty(v)
                        .unwrap_or(String::from("Error serializing value"))
                })
                .collect(),
        };

        let value_display = Paragraph::new(
            app.list_state
                .selected()
                .and_then(|selected| values.get(selected))
                .map(|entry| {
                    serde_json::to_string_pretty(entry)
                        .unwrap_or(String::from("Error serializing value"))
                })
                .unwrap_or("No value selected".to_string()),
        )
        .block(Block::default().borders(Borders::ALL).title("Value (JSON)"))
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);
        f.render_widget(value_display, inner_chunks[1]);
    }

    // Footer
    let footer = match app.mode {
        ViewMode::Normal => Paragraph::new(
            CMDS.iter().map(|(k, v)| format!("[{k}] {v}")).collect::<Vec<_>>().join(" | "),
        ),
        ViewMode::GoToPage => Paragraph::new(format!(
            "Go to page (max {}): {}",
            app.total_entries / app.count,
            app.input
        )),
    }
    .block(Block::default().borders(Borders::ALL))
    .alignment(match app.mode {
        ViewMode::Normal => Alignment::Center,
        ViewMode::GoToPage => Alignment::Left,
    })
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    f.render_widget(footer, outer_chunks[1]);
}
