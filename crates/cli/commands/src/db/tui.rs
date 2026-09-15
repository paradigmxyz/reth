//! Interactive database table listing with fallible pagination.

use crossterm::{
    cursor::Show,
    event::{self, Event, KeyCode, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use reth_db_api::{
    table::{Table, TableRow},
    RawValue,
};
use std::{
    io,
    time::{Duration, Instant},
};
use tracing::error;

/// Available keybindings for the [`DbListTUI`]
static CMDS: [(&str, &str); 6] = [
    ("q", "Quit"),
    ("↑", "Entry above"),
    ("↓", "Entry below"),
    ("←", "Previous page"),
    ("→", "Next page"),
    ("G", "Go to a specific page"),
];

#[derive(Default, Eq, PartialEq)]
pub(crate) enum ViewMode {
    /// Normal list view mode
    #[default]
    Normal,
    /// Currently wanting to go to a page
    GoToPage,
}

enum Entries<T: Table> {
    /// Pairs of [`Table::Key`] and [`RawValue<Table::Value>`]
    RawValues(Vec<(T::Key, RawValue<T::Value>)>),
    /// Pairs of [`Table::Key`] and [`Table::Value`]
    Values(Vec<TableRow<T>>),
}

impl<T: Table> Entries<T> {
    /// Creates new empty [Entries] as [`Entries::RawValues`] if `raw_values == true` and as
    /// [`Entries::Values`] if `raw == false`.
    const fn new_with_raw_values(raw_values: bool) -> Self {
        if raw_values {
            Self::RawValues(Vec::new())
        } else {
            Self::Values(Vec::new())
        }
    }

    /// Sets the internal entries [Vec], converting the [`Table::Value`] into
    /// [`RawValue<Table::Value>`] if needed.
    fn set(&mut self, new_entries: Vec<TableRow<T>>) {
        match self {
            Self::RawValues(old_entries) => {
                *old_entries =
                    new_entries.into_iter().map(|(key, value)| (key, value.into())).collect()
            }
            Self::Values(old_entries) => *old_entries = new_entries,
        }
    }

    /// Returns the length of internal [Vec].
    fn len(&self) -> usize {
        match self {
            Self::RawValues(entries) => entries.len(),
            Self::Values(entries) => entries.len(),
        }
    }

    /// Returns an iterator over keys of the internal [Vec]. For both [`Entries::RawValues`] and
    /// [`Entries::Values`], this iterator will yield [`Table::Key`].
    const fn iter_keys(&self) -> EntriesKeyIter<'_, T> {
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
    F: FnMut(usize, usize) -> eyre::Result<Vec<TableRow<T>>>,
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
    F: FnMut(usize, usize) -> eyre::Result<Vec<TableRow<T>>>,
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
        let Some(last) = self.entries.len().checked_sub(1) else {
            self.list_state.select(None);
            return;
        };
        self.list_state.select(Some(
            self.list_state.selected().map(|i| if i >= last { 0 } else { i + 1 }).unwrap_or(0),
        ));
    }

    /// Move to the previous list selection
    fn previous(&mut self) {
        let Some(last) = self.entries.len().checked_sub(1) else {
            self.list_state.select(None);
            return;
        };
        self.list_state.select(Some(
            self.list_state.selected().map(|i| if i == 0 { last } else { i - 1 }).unwrap_or(0),
        ));
    }

    fn reset(&mut self) {
        self.list_state.select((self.entries.len() > 0).then_some(0));
    }

    /// Fetch the next page of items
    fn next_page(&mut self) -> eyre::Result<()> {
        if let Some(skip) =
            self.skip.checked_add(self.count).filter(|skip| *skip < self.total_entries)
        {
            self.skip = skip;
            self.fetch_page()?;
        }
        Ok(())
    }

    /// Fetch the previous page of items
    fn previous_page(&mut self) -> eyre::Result<()> {
        if self.skip > 0 {
            self.skip = self.skip.saturating_sub(self.count);
            self.fetch_page()?;
        }
        Ok(())
    }

    /// Go to a specific page.
    fn go_to_page(&mut self, page: usize) -> eyre::Result<()> {
        // Clamp before multiplying so even the largest user-supplied page cannot overflow.
        self.skip = page.min(self.last_page()) * self.count;
        self.fetch_page()
    }

    /// Returns the last zero-based page index, or zero for an empty table.
    fn last_page(&self) -> usize {
        self.total_entries.saturating_sub(1) / self.count
    }

    /// Fetch the current page
    fn fetch_page(&mut self) -> eyre::Result<()> {
        self.entries.set((self.fetch)(self.skip, self.count)?);
        self.reset();
        Ok(())
    }

    /// Show the [`DbListTUI`] in the terminal.
    ///
    /// # Errors
    /// Returns page fetch, terminal setup, rendering, or input errors.
    pub(crate) fn run(mut self) -> eyre::Result<()> {
        // Reject a bad initial page without changing the terminal state.
        self.fetch_page()?;
        enable_raw_mode()?;
        let _restore = RestoreTerminal;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let tick_rate = Duration::from_millis(250);
        event_loop(&mut terminal, &mut self, tick_rate)
    }
}

// Restore terminal state on setup failures as well as errors after entering the event loop.
struct RestoreTerminal;

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        if let Err(err) = disable_raw_mode() {
            error!(%err, "failed to disable terminal raw mode");
        }
        if let Err(err) = execute!(io::stdout(), LeaveAlternateScreen, Show) {
            error!(%err, "failed to restore terminal screen");
        }
    }
}

/// Run the event loop
fn event_loop<B: Backend, F, T: Table>(
    terminal: &mut Terminal<B>,
    app: &mut DbListTUI<F, T>,
    tick_rate: Duration,
) -> eyre::Result<()>
where
    F: FnMut(usize, usize) -> eyre::Result<Vec<TableRow<T>>>,
    io::Error: From<B::Error>,
{
    let mut last_tick = Instant::now();
    let mut running = true;
    while running {
        // Render
        terminal.draw(|f| ui(f, app)).map_err(io::Error::from)?;

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
fn handle_event<F, T: Table>(app: &mut DbListTUI<F, T>, event: Event) -> eyre::Result<bool>
where
    F: FnMut(usize, usize) -> eyre::Result<Vec<TableRow<T>>>,
{
    if app.mode == ViewMode::GoToPage {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Enter => {
                    let input = std::mem::take(&mut app.input);
                    if let Ok(page) = input.parse() {
                        app.go_to_page(page)?;
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
        Event::Key(key) if key.kind == event::KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(true),
            KeyCode::Down => app.next(),
            KeyCode::Up => app.previous(),
            KeyCode::Right => app.next_page()?,
            KeyCode::Left => app.previous_page()?,
            KeyCode::Char('G') => {
                app.mode = ViewMode::GoToPage;
            }
            _ => {}
        },
        Event::Key(_) => {}
        Event::Mouse(e) => match e.kind {
            MouseEventKind::ScrollDown => app.next(),
            MouseEventKind::ScrollUp => app.previous(),
            // TODO: This click event can be triggered outside of the list widget.
            MouseEventKind::Down(_) => {
                let new_idx = (e.row as usize + app.list_state.offset()).saturating_sub(1);
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
fn ui<F, T: Table>(f: &mut Frame<'_>, app: &mut DbListTUI<F, T>)
where
    F: FnMut(usize, usize) -> eyre::Result<Vec<TableRow<T>>>,
{
    let outer_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(95), Constraint::Percentage(5)].as_ref())
        .split(f.area());

    // Columns
    {
        let inner_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(outer_chunks[0]);

        let key_length = format!("{}", app.skip.saturating_add(app.count).saturating_sub(1)).len();

        let formatted_keys = app
            .entries
            .iter_keys()
            .enumerate()
            .map(|(i, k)| {
                ListItem::new(format!("[{:0>width$}]: {k:?}", i + app.skip, width = key_length))
            })
            .collect::<Vec<_>>();

        let key_list = List::new(formatted_keys)
            .block(Block::default().borders(Borders::ALL).title(format!(
                "Keys (Showing entries {}-{} out of {} entries)",
                app.skip,
                app.skip.saturating_add(app.entries.len()).saturating_sub(1),
                app.total_entries
            )))
            .style(Style::default().fg(Color::White))
            .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::ITALIC))
            .highlight_symbol("➜ ");
        f.render_stateful_widget(key_list, inner_chunks[0], &mut app.list_state);

        let value_display = Paragraph::new(
            app.list_state
                .selected()
                .and_then(|selected| {
                    let maybe_serialized = match &app.entries {
                        Entries::RawValues(entries) => {
                            entries.get(selected).map(|(_, v)| serde_json::to_string(v.raw_value()))
                        }
                        Entries::Values(entries) => {
                            entries.get(selected).map(|(_, v)| serde_json::to_string_pretty(v))
                        }
                    };
                    maybe_serialized.map(|ser| {
                        ser.unwrap_or_else(|error| format!("Error serializing value: {error}"))
                    })
                })
                .unwrap_or_else(|| "No value selected".to_string()),
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
        ViewMode::GoToPage => {
            Paragraph::new(format!("Go to page (max {}): {}", app.last_page(), app.input))
        }
    }
    .block(Block::default().borders(Borders::ALL))
    .alignment(match app.mode {
        ViewMode::Normal => Alignment::Center,
        ViewMode::GoToPage => Alignment::Left,
    })
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    f.render_widget(footer, outer_chunks[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db_api::tables;

    #[test]
    fn empty_pages_have_no_selection() {
        for total_entries in [0, 42] {
            let mut app = DbListTUI::<_, tables::Headers>::new(
                |_, _| Ok(Vec::new()),
                0,
                20,
                total_entries,
                false,
            );
            app.fetch_page().unwrap();
            assert_eq!(app.list_state.selected(), None);
            for key in [KeyCode::Down, KeyCode::Up, KeyCode::Right, KeyCode::Left] {
                handle_event(&mut app, Event::Key(key.into())).unwrap();
                assert_eq!(app.list_state.selected(), None);
            }
        }
    }

    #[test]
    fn page_jumps_clamp_to_the_last_page() {
        for (total, page, expected_skip) in [
            (0, 0, 0),
            (0, usize::MAX, 0),
            (3, 0, 0),
            (3, usize::MAX, 0),
            (40, usize::MAX, 20),
            (43, 2, 40),
            (43, usize::MAX, 40),
            (usize::MAX, usize::MAX, (usize::MAX - 1) / 20 * 20),
        ] {
            let mut app = DbListTUI::<_, tables::Headers>::new(
                |skip, len| {
                    Ok((0..total.saturating_sub(skip).min(len))
                        .map(|i| ((skip + i) as u64, Default::default()))
                        .collect())
                },
                0,
                20,
                total,
                false,
            );
            app.mode = ViewMode::GoToPage;
            app.input = page.to_string();
            handle_event(&mut app, Event::Key(KeyCode::Enter.into())).unwrap();
            assert_eq!(app.skip, expected_skip);
            assert_eq!(app.entries.len(), total.saturating_sub(expected_skip).min(20));
            assert_eq!(app.list_state.selected(), (total > 0).then_some(0));
        }
    }

    #[test]
    fn next_page_near_maximum_offset_does_not_wrap() {
        let mut reads = 0;
        let mut app = DbListTUI::<_, tables::Headers>::new(
            |_, _| {
                reads += 1;
                Ok(vec![(0, Default::default())])
            },
            usize::MAX - 1,
            20,
            usize::MAX,
            false,
        );
        app.fetch_page().unwrap();
        handle_event(&mut app, Event::Key(KeyCode::Right.into())).unwrap();
        assert_eq!(app.skip, usize::MAX - 1);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| ui(frame, &mut app)).unwrap();
        assert_eq!(reads, 1);
    }

    #[test]
    fn selection_wraps_within_nonempty_pages() {
        let mut app = DbListTUI::<_, tables::Headers>::new(
            |_, _| Ok(vec![(0, Default::default()), (1, Default::default())]),
            0,
            20,
            2,
            false,
        );
        app.fetch_page().unwrap();
        for (key, selected) in [(KeyCode::Up, 1), (KeyCode::Down, 0), (KeyCode::Down, 1)] {
            handle_event(&mut app, Event::Key(key.into())).unwrap();
            assert_eq!(app.list_state.selected(), Some(selected));
        }
    }

    #[test]
    fn initial_page_error_is_returned_before_terminal_setup() {
        let app = DbListTUI::<_, tables::Headers>::new(
            |_, _| eyre::bail!("invalid table value"),
            0,
            1,
            2,
            false,
        );

        assert_eq!(app.run().unwrap_err().to_string(), "invalid table value");
    }

    #[test]
    fn pagination_events_propagate_fetch_errors() {
        for key in [KeyCode::Right, KeyCode::Left, KeyCode::Enter] {
            let mut reads = 0;
            let mut app = DbListTUI::<_, tables::Headers>::new(
                |_, _| {
                    reads += 1;
                    if reads > 1 {
                        eyre::bail!("invalid table value")
                    }
                    Ok(vec![(1, Default::default())])
                },
                1,
                1,
                3,
                false,
            );
            app.fetch_page().unwrap();
            if key == KeyCode::Enter {
                app.mode = ViewMode::GoToPage;
                app.input = "2".to_string();
            }

            let error = handle_event(&mut app, Event::Key(key.into())).unwrap_err();
            assert_eq!(error.to_string(), "invalid table value");
        }
    }
}
