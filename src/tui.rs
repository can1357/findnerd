use std::{
	io::{self, Write},
	time::{Duration, Instant},
};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use crossterm::{
	cursor::{Hide, Show},
	event::{
		self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
		Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
		MouseEventKind,
	},
	execute,
	terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
	Frame, Terminal, TerminalOptions, Viewport,
	backend::CrosstermBackend,
	layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect},
	style::{Color, Modifier, Style, Stylize},
	text::{Line, Span},
	widgets::{
		Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation,
		ScrollbarState, Table, TableState, Wrap,
	},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::search::{SearchEngine, SearchMode, SearchRequest, SearchResponse};

const BG: Color = Color::Rgb(10, 13, 20);
const SURFACE: Color = Color::Rgb(20, 25, 37);
const SURFACE_HIGH: Color = Color::Rgb(31, 38, 54);
const BORDER: Color = Color::Rgb(58, 69, 91);
const TEXT: Color = Color::Rgb(231, 235, 243);
const MUTED: Color = Color::Rgb(129, 141, 164);
const CYAN: Color = Color::Rgb(90, 205, 219);
const AMBER: Color = Color::Rgb(245, 186, 66);
const GREEN: Color = Color::Rgb(116, 214, 151);
const RED: Color = Color::Rgb(246, 116, 122);
const MAX_TUI_RESULTS: usize = 250;

/// Initial state passed from command-line flags into the alternate-screen UI.
pub struct TuiOptions {
	pub(crate) query:               String,
	pub(crate) filters:             Vec<String>,
	pub(crate) categories:          Vec<String>,
	pub(crate) excluded_categories: Vec<String>,
	pub(crate) mode:                SearchMode,
}

/// The glyph accepted with Enter in interactive mode.
pub struct Selection {
	pub(crate) glyph: String,
	pub(crate) name:  String,
}

/// Runs the event loop in an alternate buffer and restores the terminal on
/// every exit path.
pub fn run(engine: SearchEngine, options: TuiOptions) -> Result<Option<Selection>> {
	let _guard = TerminalGuard::enter()?;
	let backend = CrosstermBackend::new(io::stdout());
	let mut terminal =
		Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Fullscreen })
			.context("initializing terminal UI")?;
	terminal.clear().context("clearing alternate buffer")?;

	let mut app = App::new(engine, options)?;
	let result = app.run(&mut terminal);
	terminal.show_cursor().ok();
	result
}

struct TerminalGuard;

impl TerminalGuard {
	fn enter() -> Result<Self> {
		enable_raw_mode().context("enabling terminal raw mode")?;
		if let Err(error) = execute!(
			io::stdout(),
			EnterAlternateScreen,
			EnableBracketedPaste,
			EnableMouseCapture,
			Hide
		) {
			disable_raw_mode().ok();
			return Err(error).context("entering alternate terminal buffer");
		}
		Ok(Self)
	}
}

impl Drop for TerminalGuard {
	fn drop(&mut self) {
		disable_raw_mode().ok();
		execute!(
			io::stdout(),
			Show,
			DisableBracketedPaste,
			DisableMouseCapture,
			LeaveAlternateScreen
		)
		.ok();
	}
}

struct App {
	engine:              SearchEngine,
	query:               String,
	cursor:              usize,
	filters:             Vec<String>,
	categories:          Vec<String>,
	excluded_categories: Vec<String>,
	mode:                SearchMode,
	response:            SearchResponse,
	table:               TableState,
	results_view:        Option<ResultsView>,
	last_click:          Option<MouseClick>,
	status:              Option<Status>,
	selection:           Option<Selection>,
	quit:                bool,
}

impl App {
	fn new(engine: SearchEngine, options: TuiOptions) -> Result<Self> {
		let mut app = Self {
			engine,
			cursor: options.query.len(),
			query: options.query,
			filters: options.filters,
			categories: options.categories,
			excluded_categories: options.excluded_categories,
			mode: options.mode,
			response: SearchResponse { hits: Vec::new(), total: 0 },
			table: TableState::default(),
			status: None,
			selection: None,
			results_view: None,
			last_click: None,
			quit: false,
		};
		app.refresh()?;
		Ok(app)
	}

	fn run(
		&mut self,
		terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
	) -> Result<Option<Selection>> {
		while !self.quit {
			self.expire_status();
			terminal
				.draw(|frame| self.draw(frame))
				.context("drawing terminal UI")?;

			if !event::poll(Duration::from_millis(120)).context("polling terminal input")? {
				continue;
			}
			match event::read().context("reading terminal input")? {
				Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
					self.handle_key(key)?;
				},
				Event::Paste(text) => {
					let text = text.replace(['\n', '\r', '\t'], " ");
					self.query.insert_str(self.cursor, &text);
					self.cursor += text.len();
					self.refresh()?;
				},
				Event::Mouse(mouse) => self.handle_mouse(mouse),
				Event::Resize(..) | Event::FocusGained | Event::FocusLost => {},
				_ => {},
			}
		}
		Ok(self.selection.take())
	}

	fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
		if key.modifiers.contains(KeyModifiers::CONTROL) {
			return self.handle_control_key(key.code);
		}

		match key.code {
			KeyCode::Esc => {
				if self.query.is_empty() {
					self.quit = true;
				} else {
					self.query.clear();
					self.cursor = 0;
					self.refresh()?;
				}
			},
			KeyCode::Enter => self.accept(),
			KeyCode::Tab | KeyCode::F(2) => {
				self.mode = self.mode.next();
				self.refresh()?;
				self.set_status(format!("Ranking: {}", self.mode), CYAN);
			},
			KeyCode::BackTab => {
				self.mode = self.mode.previous();
				self.refresh()?;
				self.set_status(format!("Ranking: {}", self.mode), CYAN);
			},
			KeyCode::F(3) => self.cycle_category(1)?,
			KeyCode::Up => self.move_selection(-1),
			KeyCode::Down => self.move_selection(1),
			KeyCode::PageUp => self.move_selection(-10),
			KeyCode::PageDown => self.move_selection(10),
			KeyCode::Left => self.cursor_left(),
			KeyCode::Right => self.cursor_right(),
			KeyCode::Home => self.cursor = 0,
			KeyCode::End => self.cursor = self.query.len(),
			KeyCode::Backspace if self.delete_previous_grapheme() => {
				self.refresh()?;
			},
			KeyCode::Delete if self.delete_next_grapheme() => {
				self.refresh()?;
			},
			KeyCode::Char(character)
				if !key
					.modifiers
					.intersects(KeyModifiers::ALT | KeyModifiers::SUPER) =>
			{
				self.query.insert(self.cursor, character);
				self.cursor += character.len_utf8();
				self.refresh()?;
			},
			_ => {},
		}
		Ok(())
	}

	fn handle_control_key(&mut self, code: KeyCode) -> Result<()> {
		match code {
			KeyCode::Char('c' | 'q') => self.quit = true,
			KeyCode::Char('a') => self.cursor = 0,
			KeyCode::Char('e') => self.cursor = self.query.len(),
			KeyCode::Char('u') => {
				self.query.clear();
				self.cursor = 0;
				self.refresh()?;
			},
			KeyCode::Char('w') if self.delete_previous_word() => {
				self.refresh()?;
			},
			KeyCode::Char('j') => self.move_selection(1),
			KeyCode::Char('k') => self.move_selection(-1),
			KeyCode::Char('g') => self.cycle_category(1)?,
			KeyCode::Char('t') => {
				self.mode = self.mode.next();
				self.refresh()?;
				self.set_status(format!("Ranking: {}", self.mode), CYAN);
			},
			KeyCode::Char('y') => self.copy_selected()?,
			_ => {},
		}
		Ok(())
	}

	fn handle_mouse(&mut self, mouse: MouseEvent) {
		match mouse.kind {
			MouseEventKind::Moved => {
				if let Some(index) = self.result_at(mouse.column, mouse.row) {
					self.table.select(Some(index));
				}
			},
			MouseEventKind::Down(MouseButton::Left) => {
				let Some(index) = self.result_at(mouse.column, mouse.row) else {
					self.last_click = None;
					return;
				};
				self.table.select(Some(index));
				let now = Instant::now();
				let double_click = is_double_click(self.last_click.as_ref(), index, now);
				self.last_click = Some(MouseClick { index, at: now });
				if double_click {
					self.accept();
				}
			},
			MouseEventKind::ScrollUp if self.mouse_over_results(mouse.column, mouse.row) => {
				self.move_selection(-3);
			},
			MouseEventKind::ScrollDown if self.mouse_over_results(mouse.column, mouse.row) => {
				self.move_selection(3);
			},
			_ => {},
		}
	}

	fn result_at(&self, column: u16, row: u16) -> Option<usize> {
		self
			.results_view?
			.index_at(column, row, self.response.hits.len())
	}

	fn mouse_over_results(&self, column: u16, row: u16) -> bool {
		self
			.results_view
			.is_some_and(|view| view.contains(column, row))
	}

	fn refresh(&mut self) -> Result<()> {
		let request = SearchRequest {
			query:               self.query.clone(),
			filters:             self.filters.clone(),
			categories:          self.categories.clone(),
			excluded_categories: self.excluded_categories.clone(),
			mode:                self.mode,
			limit:               MAX_TUI_RESULTS,
		};
		self.response = self.engine.search(&request)?;
		self
			.table
			.select((!self.response.hits.is_empty()).then_some(0));
		Ok(())
	}

	fn accept(&mut self) {
		let Some(index) = self.table.selected() else {
			return;
		};
		let icon = self.engine.icon(self.response.hits[index].icon);
		self.selection = Some(Selection { glyph: icon.glyph.clone(), name: icon.name.clone() });
		self.quit = true;
	}

	fn copy_selected(&mut self) -> Result<()> {
		let Some(index) = self.table.selected() else {
			self.set_status("Nothing selected".to_owned(), RED);
			return Ok(());
		};
		let icon = self.engine.icon(self.response.hits[index].icon);
		let encoded = STANDARD.encode(icon.glyph.as_bytes());
		let sequence = format!("\x1b]52;c;{encoded}\x1b\\");
		io::stdout()
			.write_all(sequence.as_bytes())
			.context("writing OSC 52 clipboard sequence")?;
		io::stdout()
			.flush()
			.context("flushing clipboard sequence")?;
		self.set_status(format!("Copied {}", icon.name), GREEN);
		Ok(())
	}

	fn cycle_category(&mut self, direction: i32) -> Result<()> {
		let category_count = self.engine.categories().len();
		let current = if self.categories.len() == 1 {
			self
				.engine
				.categories()
				.iter()
				.position(|category| category.slug == self.categories[0])
				.map_or(0, |index| index + 1)
		} else {
			0
		};
		let option_count = category_count + 1;
		let next = if direction >= 0 {
			(current + 1) % option_count
		} else if current == 0 {
			option_count - 1
		} else {
			current - 1
		};
		self.categories = if next == 0 {
			Vec::new()
		} else {
			vec![self.engine.categories()[next - 1].slug.clone()]
		};
		self.excluded_categories.clear();
		self.refresh()?;
		self.set_status(format!("Category: {}", self.category_label()), AMBER);
		Ok(())
	}

	fn move_selection(&mut self, delta: i32) {
		if self.response.hits.is_empty() {
			return;
		}
		let current = self.table.selected().unwrap_or(0);
		let maximum = self.response.hits.len() - 1;
		let next = if delta.is_negative() {
			current.saturating_sub(usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX))
		} else {
			current
				.saturating_add(usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX))
				.min(maximum)
		};
		self.table.select(Some(next));
	}

	fn cursor_left(&mut self) {
		if let Some((index, _)) = self.query[..self.cursor].grapheme_indices(true).next_back() {
			self.cursor = index;
		}
	}

	fn cursor_right(&mut self) {
		if let Some(grapheme) = self.query[self.cursor..].graphemes(true).next() {
			self.cursor += grapheme.len();
		}
	}

	fn delete_previous_grapheme(&mut self) -> bool {
		let Some((start, _)) = self.query[..self.cursor].grapheme_indices(true).next_back() else {
			return false;
		};
		self.query.replace_range(start..self.cursor, "");
		self.cursor = start;
		true
	}

	fn delete_next_grapheme(&mut self) -> bool {
		let Some(grapheme) = self.query[self.cursor..].graphemes(true).next() else {
			return false;
		};
		self
			.query
			.replace_range(self.cursor..self.cursor + grapheme.len(), "");
		true
	}

	fn delete_previous_word(&mut self) -> bool {
		if self.cursor == 0 {
			return false;
		}
		let prefix = &self.query[..self.cursor];
		let trimmed = prefix.trim_end_matches(char::is_whitespace);
		let start = trimmed
			.rfind(char::is_whitespace)
			.map_or(0, |index| index + trimmed[index..].chars().next().map_or(0, char::len_utf8));
		self.query.replace_range(start..self.cursor, "");
		self.cursor = start;
		true
	}

	fn category_label(&self) -> String {
		match self.categories.as_slice() {
			[] if self.excluded_categories.is_empty() => "all".to_owned(),
			[] => format!("all except {}", self.excluded_categories.join(", ")),
			[slug] => self
				.engine
				.categories()
				.iter()
				.find(|category| &category.slug == slug)
				.map_or_else(|| slug.clone(), |category| category.name.clone()),
			many => format!("{} selected", many.len()),
		}
	}

	fn category_badge(&self) -> String {
		match self.categories.as_slice() {
			[] if self.excluded_categories.is_empty() => "all".to_owned(),
			[] if self.excluded_categories.len() == 1 => {
				format!("all −{}", self.excluded_categories[0])
			},
			[] => format!("all −{}", self.excluded_categories.len()),
			[slug] => slug.clone(),
			many => format!("{} cats", many.len()),
		}
	}

	fn set_status(&mut self, message: String, color: Color) {
		self.status =
			Some(Status { message, color, expires: Instant::now() + Duration::from_secs(2) });
	}

	fn expire_status(&mut self) {
		if self
			.status
			.as_ref()
			.is_some_and(|status| Instant::now() >= status.expires)
		{
			self.status = None;
		}
	}

	fn draw(&mut self, frame: &mut Frame<'_>) {
		self.results_view = None;
		frame.render_widget(Block::new().style(Style::default().bg(BG)), frame.area());
		let area = frame.area().inner(Margin { horizontal: 2, vertical: 1 });
		if area.width < 42 || area.height < 15 {
			Self::draw_too_small(frame, area);
			return;
		}

		let sections = Layout::default()
			.direction(Direction::Vertical)
			.constraints([
				Constraint::Length(2),
				Constraint::Length(3),
				Constraint::Length(2),
				Constraint::Min(6),
				Constraint::Length(2),
			])
			.split(area);
		self.draw_header(frame, sections[0]);
		self.draw_input(frame, sections[1]);
		self.draw_controls(frame, sections[2]);
		self.draw_content(frame, sections[3]);
		self.draw_footer(frame, sections[4]);
	}

	fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
		let columns = Layout::default()
			.direction(Direction::Horizontal)
			.constraints([Constraint::Min(24), Constraint::Length(34)])
			.split(area);
		frame.render_widget(
			Paragraph::new(Line::from(vec![
				Span::styled("find", Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
				Span::styled("nerd", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
				Span::styled("  semantic glyph finder", Style::default().fg(MUTED)),
			])),
			columns[0],
		);
		let icon_count = self
			.engine
			.categories()
			.iter()
			.map(|category| category.icon_count)
			.sum::<usize>();
		frame.render_widget(
			Paragraph::new(Line::from(vec![
				Span::styled(format_count(icon_count), Style::default().fg(AMBER)),
				Span::styled(" glyphs  ·  local index", Style::default().fg(MUTED)),
			]))
			.alignment(Alignment::Right),
			columns[1],
		);
	}

	fn draw_input(&self, frame: &mut Frame<'_>, area: Rect) {
		let block = panel()
			.border_style(Style::default().fg(CYAN))
			.title(Span::styled(" QUERY ", Style::default().fg(CYAN).add_modifier(Modifier::BOLD)));
		let inner = block.inner(area);
		frame.render_widget(block, area);
		let available = usize::from(inner.width.saturating_sub(2));
		let (visible, cursor_column) = input_window(&self.query, self.cursor, available);
		let line = if self.query.is_empty() {
			Line::from(vec![
				Span::styled("› ", Style::default().fg(CYAN)),
				Span::styled(
					"Search intent: “delete file”, “night weather”, “python”…",
					Style::default().fg(MUTED),
				),
			])
		} else {
			Line::from(vec![
				Span::styled("› ", Style::default().fg(CYAN)),
				Span::styled(visible, Style::default().fg(TEXT)),
			])
		};
		frame.render_widget(Paragraph::new(line).style(Style::default().bg(SURFACE)), inner);
		let cursor_x = inner
			.x
			.saturating_add(2)
			.saturating_add(u16::try_from(cursor_column).unwrap_or(u16::MAX));
		frame.set_cursor_position(Position::new(
			cursor_x.min(inner.right().saturating_sub(1)),
			inner.y,
		));
	}

	fn draw_controls(&self, frame: &mut Frame<'_>, area: Rect) {
		let columns = Layout::default()
			.direction(Direction::Horizontal)
			.constraints([Constraint::Min(30), Constraint::Length(24)])
			.split(area);
		let mut spans = vec![Span::styled(" MODE ", Style::default().fg(MUTED))];
		for mode in [SearchMode::Hybrid, SearchMode::Semantic, SearchMode::Bm25, SearchMode::Match] {
			let style = if mode == self.mode {
				Style::default()
					.fg(BG)
					.bg(CYAN)
					.add_modifier(Modifier::BOLD)
			} else {
				Style::default().fg(MUTED).bg(SURFACE)
			};
			spans.push(Span::styled(format!(" {mode} "), style));
		}
		frame.render_widget(Paragraph::new(Line::from(spans)), columns[0]);
		frame.render_widget(
			Paragraph::new(Line::from(vec![
				Span::styled("CATEGORY  ", Style::default().fg(MUTED)),
				Span::styled(self.category_badge(), Style::default().fg(AMBER)),
				Span::styled("  ^G", Style::default().fg(BORDER)),
			]))
			.alignment(Alignment::Right),
			columns[1],
		);
	}

	fn draw_content(&mut self, frame: &mut Frame<'_>, area: Rect) {
		if area.width >= 92 {
			let columns = Layout::default()
				.direction(Direction::Horizontal)
				.constraints([Constraint::Percentage(61), Constraint::Percentage(39)])
				.spacing(1)
				.split(area);
			self.draw_results(frame, columns[0]);
			self.draw_preview(frame, columns[1]);
		} else {
			self.draw_results(frame, area);
		}
	}

	fn draw_results(&mut self, frame: &mut Frame<'_>, area: Rect) {
		let title = if self.response.total > self.response.hits.len() {
			format!(" RESULTS  {} shown · {} matches ", self.response.hits.len(), self.response.total)
		} else {
			format!(" RESULTS  {} ", self.response.hits.len())
		};
		let block = panel()
			.title(Span::styled(title, Style::default().fg(MUTED).add_modifier(Modifier::BOLD)));

		if self.response.hits.is_empty() {
			self.results_view = None;
			let inner = block.inner(area);
			frame.render_widget(block, area);
			frame.render_widget(
				Paragraph::new(vec![
					Line::from(Span::styled("No glyphs found", Style::default().fg(TEXT).bold())),
					Line::from(Span::styled(
						"Try broader words, semantic mode, or clear a filter.",
						Style::default().fg(MUTED),
					)),
				])
				.alignment(Alignment::Center)
				.wrap(Wrap { trim: true }),
				centered(inner, 2),
			);
			return;
		}

		let wide = area.width >= 66;
		let rows = self.response.hits.iter().map(|hit| {
			let icon = self.engine.icon(hit.icon);
			let category = self.engine.category(icon.category);
			let mut cells = vec![
				Cell::from(icon.glyph.clone()).style(Style::default().fg(AMBER).bold()),
				Cell::from(icon.name.clone()).style(Style::default().fg(TEXT)),
			];
			if wide {
				cells.push(Cell::from(category.slug.clone()).style(Style::default().fg(MUTED)));
			}
			cells.push(
				Cell::from(format!("{:>3.0}%", hit.score * 100.0)).style(Style::default().fg(CYAN)),
			);
			Row::new(cells)
		});
		let widths = if wide {
			vec![
				Constraint::Length(3),
				Constraint::Min(18),
				Constraint::Length(9),
				Constraint::Length(5),
			]
		} else {
			vec![Constraint::Length(3), Constraint::Min(18), Constraint::Length(5)]
		};
		let table = Table::new(rows, widths)
			.block(block)
			.row_highlight_style(
				Style::default()
					.bg(SURFACE_HIGH)
					.add_modifier(Modifier::BOLD),
			)
			.highlight_symbol("▌ ")
			.column_spacing(1);
		frame.render_stateful_widget(table, area, &mut self.table);
		self.results_view = Some(ResultsView {
			area:   area.inner(Margin { horizontal: 1, vertical: 1 }),
			offset: self.table.offset(),
		});

		let mut scrollbar_state =
			ScrollbarState::new(self.response.hits.len()).position(self.table.selected().unwrap_or(0));
		frame.render_stateful_widget(
			Scrollbar::new(ScrollbarOrientation::VerticalRight)
				.thumb_style(Style::default().fg(BORDER))
				.track_style(Style::default().fg(SURFACE)),
			area.inner(Margin { horizontal: 0, vertical: 1 }),
			&mut scrollbar_state,
		);
	}

	fn draw_preview(&self, frame: &mut Frame<'_>, area: Rect) {
		let block = panel()
			.title(Span::styled(" PREVIEW ", Style::default().fg(MUTED).add_modifier(Modifier::BOLD)));
		let inner = block.inner(area);
		frame.render_widget(block, area);
		let Some(selected) = self.table.selected() else {
			return;
		};
		let hit = self.response.hits[selected];
		let icon = self.engine.icon(hit.icon);
		let category = self.engine.category(icon.category);
		let sections = Layout::default()
			.direction(Direction::Vertical)
			.constraints([
				Constraint::Length(5),
				Constraint::Length(2),
				Constraint::Length(3),
				Constraint::Min(2),
			])
			.split(inner);
		frame.render_widget(
			Paragraph::new(icon.glyph.as_str())
				.alignment(Alignment::Center)
				.style(
					Style::default()
						.fg(AMBER)
						.bg(SURFACE)
						.add_modifier(Modifier::BOLD),
				),
			centered(sections[0], 1),
		);
		frame.render_widget(
			Paragraph::new(icon.name.as_str())
				.alignment(Alignment::Center)
				.style(Style::default().fg(TEXT).bold()),
			sections[1],
		);
		frame.render_widget(
			Paragraph::new(vec![
				Line::from(vec![
					Span::styled(format!("U+{:04X}", icon.codepoint), Style::default().fg(CYAN)),
					Span::styled("  ·  ", Style::default().fg(BORDER)),
					Span::styled(&category.name, Style::default().fg(MUTED)),
				]),
				Line::from(vec![
					Span::styled("score  ", Style::default().fg(MUTED)),
					Span::styled(
						format!("{:.0}% {}", hit.score * 100.0, self.mode),
						Style::default().fg(GREEN),
					),
				]),
			])
			.alignment(Alignment::Center),
			sections[2],
		);
		let aliases = if icon.aliases.is_empty() {
			"No semantic aliases".to_owned()
		} else {
			icon.aliases.replace(' ', "  ·  ")
		};
		frame.render_widget(
			Paragraph::new(vec![
				Line::from(Span::styled("RELATED", Style::default().fg(MUTED).bold())),
				Line::from(Span::styled(aliases, Style::default().fg(TEXT))),
			])
			.wrap(Wrap { trim: true }),
			sections[3].inner(Margin { horizontal: 2, vertical: 0 }),
		);
	}

	fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
		let help = Line::from(vec![
			key("↑↓/mouse"),
			hint(" move  "),
			key("Enter/2×"),
			hint(" pick  "),
			key("^Y"),
			hint(" copy  "),
			key("Tab"),
			hint(" mode  "),
			key("Esc"),
			hint(" clear/quit"),
		]);
		let message = self.status.as_ref().map_or_else(
			|| {
				(!self.filters.is_empty()).then(|| {
					Line::from(Span::styled(
						format!("{} filter(s) active", self.filters.len()),
						Style::default().fg(AMBER),
					))
				})
			},
			|status| {
				Some(Line::from(Span::styled(&status.message, Style::default().fg(status.color))))
			},
		);
		let Some(message) = message else {
			frame.render_widget(Paragraph::new(help), area);
			return;
		};
		let columns = Layout::default()
			.direction(Direction::Horizontal)
			.constraints([Constraint::Min(40), Constraint::Length(20)])
			.split(area);
		frame.render_widget(Paragraph::new(help), columns[0]);
		frame.render_widget(Paragraph::new(message).alignment(Alignment::Right), columns[1]);
	}

	fn draw_too_small(frame: &mut Frame<'_>, area: Rect) {
		frame.render_widget(Clear, area);
		frame.render_widget(
			Paragraph::new(vec![
				Line::from(Span::styled("findnerd", Style::default().fg(CYAN).bold())),
				Line::from(Span::styled("Terminal needs at least 42 × 15", Style::default().fg(MUTED))),
			])
			.alignment(Alignment::Center),
			centered(area, 2),
		);
	}
}

#[derive(Clone, Copy)]
struct ResultsView {
	area:   Rect,
	offset: usize,
}
impl ResultsView {
	const fn contains(self, column: u16, row: u16) -> bool {
		column >= self.area.x
			&& column < self.area.right()
			&& row >= self.area.y
			&& row < self.area.bottom()
	}

	fn index_at(self, column: u16, row: u16, result_count: usize) -> Option<usize> {
		if !self.contains(column, row) {
			return None;
		}
		let index = self.offset + usize::from(row - self.area.y);
		(index < result_count).then_some(index)
	}
}

struct MouseClick {
	index: usize,
	at:    Instant,
}
fn is_double_click(previous: Option<&MouseClick>, index: usize, now: Instant) -> bool {
	previous.is_some_and(|click| {
		click.index == index && now.saturating_duration_since(click.at) <= Duration::from_millis(450)
	})
}

struct Status {
	message: String,
	color:   Color,
	expires: Instant,
}

fn panel<'a>() -> Block<'a> {
	Block::default()
		.borders(Borders::ALL)
		.border_type(BorderType::Rounded)
		.border_style(Style::default().fg(BORDER))
		.style(Style::default().bg(SURFACE))
}

fn key(value: &'static str) -> Span<'static> {
	Span::styled(value, Style::default().fg(CYAN).add_modifier(Modifier::BOLD))
}

fn hint(value: &'static str) -> Span<'static> {
	Span::styled(value, Style::default().fg(MUTED))
}

fn centered(area: Rect, height: u16) -> Rect {
	Layout::default()
		.direction(Direction::Vertical)
		.constraints([
			Constraint::Fill(1),
			Constraint::Length(height.min(area.height)),
			Constraint::Fill(1),
		])
		.split(area)[1]
}

fn format_count(count: usize) -> String {
	let digits = count.to_string();
	let mut result = String::with_capacity(digits.len() + digits.len() / 3);
	for (index, character) in digits.chars().enumerate() {
		if index > 0 && (digits.len() - index).is_multiple_of(3) {
			result.push(',');
		}
		result.push(character);
	}
	result
}

fn input_window(query: &str, cursor: usize, width: usize) -> (String, usize) {
	if width == 0 {
		return (String::new(), 0);
	}

	let mut start = cursor;
	let target_before = width.saturating_sub(2);
	for (index, _) in query[..cursor].grapheme_indices(true).rev() {
		let candidate = &query[index..cursor];
		if UnicodeWidthStr::width(candidate) > target_before {
			break;
		}
		start = index;
	}
	let leading = usize::from(start > 0);
	let cursor_column = leading + UnicodeWidthStr::width(&query[start..cursor]);
	let mut visible = String::new();
	if start > 0 {
		visible.push('…');
	}
	for grapheme in query[start..].graphemes(true) {
		if UnicodeWidthStr::width(visible.as_str()) + UnicodeWidthStr::width(grapheme) > width {
			break;
		}
		visible.push_str(grapheme);
	}
	(visible, cursor_column.min(width.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn input_window_keeps_cursor_visible() {
		let (visible, cursor) = input_window("configuration preferences", 25, 12);
		assert!(visible.starts_with('…'));
		assert!(cursor < 12);
		assert!(UnicodeWidthStr::width(visible.as_str()) <= 12);
	}

	#[test]
	fn count_uses_readable_grouping() {
		assert_eq!(format_count(10_764), "10,764");
	}
	#[test]
	fn mouse_rows_include_the_scroll_offset_and_respect_boundaries() {
		let view = ResultsView { area: Rect::new(4, 8, 30, 5), offset: 7 };

		assert_eq!(view.index_at(4, 8, 20), Some(7));
		assert_eq!(view.index_at(12, 12, 20), Some(11));
		assert_eq!(view.index_at(12, 13, 20), None);
		assert_eq!(view.index_at(34, 8, 20), None);
	}

	#[test]
	fn double_click_requires_the_same_row_within_the_interval() {
		let now = Instant::now();
		let recent =
			MouseClick { index: 3, at: now.checked_sub(Duration::from_millis(400)).unwrap() };
		let old =
			MouseClick { index: 3, at: now.checked_sub(Duration::from_millis(451)).unwrap() };

		assert!(is_double_click(Some(&recent), 3, now));
		assert!(!is_double_click(Some(&recent), 4, now));
		assert!(!is_double_click(Some(&old), 3, now));
	}
}
