use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use reth_db::table::Table;
use std::{
    collections::HashMap,
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
static CMDS: [(&'static str, &'static str); 3] =
    [("q", "Quit"), ("up", "Entry Above"), ("down", "Entry Below")];

#[derive(Default)]
pub(crate) struct DbListTUI<T: Table> {
    /// The state of the key list.
    pub(crate) state: ListState,
    /// The starting index of the key list in the DB.
    pub(crate) start: usize,
    /// The total number of entries in the database
    pub(crate) total_entries: usize,
    /// Entries to show in the TUI.
    pub(crate) entries: HashMap<T::Key, T::Value>,
}

impl<T: Table> DbListTUI<T> {
    fn new(entries: HashMap<T::Key, T::Value>, start: usize, total_entries: usize) -> Self {
        Self { state: ListState::default(), start, total_entries, entries }
    }

    /// Move to the next list selection
    fn next(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.entries.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    /// Move to the previous list selection
    fn previous(&mut self) {
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.entries.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    /// Show the [DbListTUI] in the terminal.
    pub(crate) fn show_tui(
        entries: HashMap<T::Key, T::Value>,
        start: usize,
        total_entries: usize,
    ) -> eyre::Result<()> {
        // setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // create app and run it
        let tick_rate = Duration::from_millis(250);
        let mut app = DbListTUI::<T>::new(entries, start, total_entries);
        app.state.select(Some(0));
        let res = run(&mut terminal, app, tick_rate);

        // restore terminal
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
        terminal.show_cursor()?;

        if let Err(err) = res {
            error!("{:?}", err)
        }

        Ok(())
    }
}

fn run<B: Backend, T: Table>(
    terminal: &mut Terminal<B>,
    mut app: DbListTUI<T>,
    tick_rate: Duration,
) -> io::Result<()> {
    let mut last_tick = Instant::now();
    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        let timeout =
            tick_rate.checked_sub(last_tick.elapsed()).unwrap_or_else(|| Duration::from_secs(0));
        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Down => app.next(),
                    KeyCode::Up => app.previous(),
                    _ => {}
                }
            }
        }
        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

fn ui<B: Backend, T: Table>(f: &mut Frame<'_, B>, app: &mut DbListTUI<T>) {
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

        let formatted_keys = app
            .entries
            .keys()
            .enumerate()
            .map(|(i, k)| ListItem::new(format!("[{}] - {k:?}", i + app.start)))
            .collect::<Vec<ListItem<'_>>>();

        let key_list = List::new(formatted_keys)
            .block(Block::default().borders(Borders::ALL).title(format!(
                "Keys (Showing range [{}, {}] out of {} entries)",
                app.start,
                app.start + app.entries.len() - 1,
                app.total_entries
            )))
            .style(Style::default().fg(Color::White))
            .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::ITALIC))
            .highlight_symbol(">> ")
            .start_corner(Corner::TopLeft);
        f.render_stateful_widget(key_list, inner_chunks[0], &mut app.state);

        let value_display = Paragraph::new(
            serde_json::to_string_pretty(
                &app.entries.values().collect::<Vec<_>>()[app.state.selected().unwrap_or(0)],
            )
            .unwrap_or(String::from("Error serializing value!")),
        )
        .block(Block::default().borders(Borders::ALL).title("Value (JSON)"))
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);
        f.render_widget(value_display, inner_chunks[1]);
    }

    // Footer
    let footer = Paragraph::new(
        CMDS.iter().map(|(k, v)| format!("[{k}] {v}")).collect::<Vec<_>>().join(" | "),
    )
    .block(Block::default().borders(Borders::ALL))
    .alignment(Alignment::Center)
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    f.render_widget(footer, outer_chunks[1]);
}
