use crate::parser::TorrentFile;
use crate::tracker::TrackerResponse;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap},
};
use std::{
    io::{self, Stdout},
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub enum UIEvent {
    TorrentParsed(TorrentFile),
    TrackerResponse(TrackerResponse),
    ConnectingToPeer(SocketAddr),
    PeerConnected(SocketAddr),
    PeerConnectionFailed(SocketAddr, String),
    DownloadStarted,
    PieceCompleted(u32, usize, usize), // piece_index, completed_count, total_count
    DownloadComplete,
    DownloadStopped,
    Error(String),
}

#[derive(Default)]
struct UIState {
    torrent: Option<TorrentFile>,
    tracker_response: Option<TrackerResponse>,
    current_peer: Option<SocketAddr>,
    connected_peer: Option<SocketAddr>,
    download_started: bool,
    completed_pieces: usize,
    total_pieces: usize,
    last_piece_time: Option<Instant>,
    download_speed: f64, // pieces per second
    eta: Option<Duration>,
    error_message: Option<String>,
    log_messages: Vec<String>,
    start_time: Option<Instant>,
    current_piece_index: Option<u32>,
    pieces_per_minute: f64,
    bytes_downloaded: u64,
    bytes_per_second: f64,
}

impl UIState {
    fn update_progress(&mut self, piece_index: u32, completed: usize, total: usize) {
        let now = Instant::now();

        // Update basic stats
        self.completed_pieces = completed;
        self.total_pieces = total;
        self.current_piece_index = Some(piece_index);

        // Calculate bytes downloaded (assuming pieces are mostly full size)
        if let Some(ref torrent) = self.torrent {
            self.bytes_downloaded = completed as u64 * torrent.info.piece_length as u64;
        }

        // Calculate speed and ETA
        if let Some(start) = self.start_time {
            let elapsed = now.duration_since(start).as_secs_f64();
            if elapsed > 0.0 && completed > 0 {
                // Calculate overall average speed
                self.download_speed = completed as f64 / elapsed;
                self.pieces_per_minute = self.download_speed * 60.0;

                // Calculate bytes per second
                if let Some(ref torrent) = self.torrent {
                    self.bytes_per_second = self.download_speed * torrent.info.piece_length as f64;
                }

                // Calculate ETA
                let remaining = total - completed;
                if self.download_speed > 0.0 && remaining > 0 {
                    let eta_seconds = remaining as f64 / self.download_speed;
                    self.eta = Some(Duration::from_secs_f64(eta_seconds));
                }
            }
        }

        self.last_piece_time = Some(now);

        // Only log milestone pieces (every 100 pieces or multiples of 5% progress)
        if completed == 1 || piece_index % 100 == 0 || (completed * 20) % total == 0 {
            let _percentage = if total > 0 {
                (completed * 100) / total
            } else {
                0
            };
        }
    }

    fn add_log(&mut self, message: String) {
        self.log_messages.push(format!("{}", message));
        // Keep only last 50 messages
        if self.log_messages.len() > 50 {
            self.log_messages.remove(0);
        }
    }

    fn progress_percentage(&self) -> f64 {
        if self.total_pieces > 0 {
            (self.completed_pieces as f64 / self.total_pieces as f64) * 100.0
        } else {
            0.0
        }
    }
}

pub struct UI {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    state: Arc<Mutex<UIState>>,
    event_rx: Receiver<UIEvent>,
    event_tx: Sender<UIEvent>,
    should_quit: Arc<AtomicBool>,
}

impl UI {
    pub fn new() -> Result<Self, io::Error> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let (event_tx, event_rx) = mpsc::channel();
        let should_quit = Arc::new(AtomicBool::new(false));

        Ok(UI {
            terminal,
            state: Arc::new(Mutex::new(UIState::default())),
            event_rx,
            event_tx,
            should_quit,
        })
    }

    pub fn get_event_sender(&self) -> Sender<UIEvent> {
        self.event_tx.clone()
    }

    pub fn run(&mut self) -> Result<(), io::Error> {
        // Start input handling thread
        let should_quit = self.should_quit.clone();
        thread::spawn(move || {
            loop {
                if should_quit.load(Ordering::Relaxed) {
                    break;
                }

                if event::poll(Duration::from_millis(100)).unwrap() {
                    if let Ok(Event::Key(key)) = event::read() {
                        if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                            should_quit.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
        });

        // Initial message
        {
            let mut state = self.state.lock().unwrap();
            state.add_log("BitTorrent client started. Press 'q' or ESC to quit.".to_string());
        }

        // Main UI loop
        loop {
            if self.should_quit.load(Ordering::Relaxed) {
                break;
            }

            // Handle events
            while let Ok(event) = self.event_rx.try_recv() {
                self.handle_event(event);
            }

            // Draw UI
            let state = self.state.lock().unwrap();
            self.terminal.draw(|f| Self::draw_ui(f, &state))?;
            drop(state);

            // Small delay to prevent busy waiting
            thread::sleep(Duration::from_millis(50));
        }

        // Cleanup
        self.restore_terminal()?;
        Ok(())
    }

    fn handle_event(&mut self, event: UIEvent) {
        let mut state = self.state.lock().unwrap();

        match event {
            UIEvent::TorrentParsed(torrent) => {
                state.add_log(format!("Torrent parsed: {}", torrent.info.name));
                state.add_log(format!("  Total size: {} bytes", torrent.total_size()));
                state.add_log(format!(
                    "  Piece length: {} bytes",
                    torrent.info.piece_length
                ));
                state.add_log(format!("  Number of pieces: {}", torrent.info.pieces.len()));
                state.total_pieces = torrent.info.pieces.len();
                state.torrent = Some(torrent);
            }
            UIEvent::TrackerResponse(response) => {
                state.add_log(format!(
                    "Tracker response: {} peers found",
                    response.peers.len()
                ));
                state.add_log(format!(
                    "  Seeders: {}, Leechers: {}",
                    response.complete, response.incomplete
                ));
                state.add_log(format!("  Interval: {} seconds", response.interval));
                state.tracker_response = Some(response);
            }
            UIEvent::ConnectingToPeer(addr) => {
                state.add_log(format!("Connecting to peer: {}", addr));
                state.current_peer = Some(addr);
            }
            UIEvent::PeerConnected(addr) => {
                state.add_log(format!("Connected to peer: {}", addr));
                state.connected_peer = Some(addr);
            }
            UIEvent::PeerConnectionFailed(addr, error) => {
                state.add_log(format!("Failed to connect to {}: {}", addr, error));
                state.current_peer = None;
            }
            UIEvent::DownloadStarted => {
                state.download_started = true;
                state.start_time = Some(Instant::now());
            }
            UIEvent::PieceCompleted(piece_index, completed, total) => {
                state.update_progress(piece_index, completed, total);
            }
            UIEvent::DownloadComplete => {
                state.add_log("Download completed successfully!".to_string());
            }
            UIEvent::DownloadStopped => {
                state.add_log("Download stopped by user.".to_string());
            }
            UIEvent::Error(error) => {
                state.add_log(format!("Error: {}", error));
                state.error_message = Some(error);
            }
        }
    }

    fn draw_ui(f: &mut Frame, state: &UIState) {
        // Main layout
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title
                Constraint::Length(8), // Torrent info
                Constraint::Length(6), // Connection info
                Constraint::Length(6), // Progress (increased from 4)
                Constraint::Min(5),    // Logs
                Constraint::Length(1), // Help
            ])
            .split(f.size());

        // Title
        let title = Paragraph::new("Il Pleut - BitTorrent Client")
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(title, chunks[0]);

        // Torrent info
        Self::draw_torrent_info(f, chunks[1], state);

        // Connection info
        Self::draw_connection_info(f, chunks[2], state);

        // Progress
        Self::draw_progress(f, chunks[3], state);

        // Logs
        Self::draw_logs(f, chunks[4], state);

        // Help
        let help = Paragraph::new("Press 'q' or ESC to quit")
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);
        f.render_widget(help, chunks[5]);
    }

    fn draw_torrent_info(f: &mut Frame, area: Rect, state: &UIState) {
        let info = if let Some(ref torrent) = state.torrent {
            vec![
                Line::from(vec![
                    Span::styled(
                        "File: ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(&torrent.info.name),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Total Size: ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format_bytes(torrent.total_size())),
                    Span::styled("  (", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", torrent.total_size())),
                    Span::styled(" bytes)", Style::default().fg(Color::Gray)),
                ]),
                Line::from(vec![
                    Span::styled("Pieces: ", Style::default().fg(Color::Yellow)),
                    Span::raw(format!("{}", torrent.info.pieces.len())),
                    Span::styled("  Size: ", Style::default().fg(Color::Yellow)),
                    Span::raw(format_bytes(torrent.info.piece_length as u64)),
                ]),
                Line::from(vec![
                    Span::styled("Tracker: ", Style::default().fg(Color::Blue)),
                    Span::raw(&torrent.announce),
                ]),
                Line::from(vec![
                    Span::styled("Info Hash: ", Style::default().fg(Color::Magenta)),
                    Span::raw(format!("{:02x}", torrent.info_hash[0])),
                    Span::raw(format!("{:02x}", torrent.info_hash[1])),
                    Span::raw(format!("{:02x}", torrent.info_hash[2])),
                    Span::raw(format!("{:02x}", torrent.info_hash[3])),
                    Span::styled("...", Style::default().fg(Color::Gray)),
                ]),
            ]
        } else {
            vec![Line::from(vec![Span::styled(
                "Loading torrent...",
                Style::default().fg(Color::Yellow),
            )])]
        };

        let paragraph = Paragraph::new(info)
            .block(
                Block::default()
                    .title("Torrent Information")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: true });
        f.render_widget(paragraph, area);
    }

    fn draw_connection_info(f: &mut Frame, area: Rect, state: &UIState) {
        let mut lines = Vec::new();

        if let Some(ref response) = state.tracker_response {
            lines.push(Line::from(vec![
                Span::styled("Seeders: ", Style::default().fg(Color::Green)),
                Span::raw(format!("{}", response.complete)),
                Span::styled("  Leechers: ", Style::default().fg(Color::Yellow)),
                Span::raw(format!("{}", response.incomplete)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Available Peers: ", Style::default().fg(Color::Blue)),
                Span::raw(format!("{}", response.peers.len())),
            ]));
        }

        if let Some(addr) = state.connected_peer {
            lines.push(Line::from(vec![
                Span::styled("Connected to: ", Style::default().fg(Color::Green)),
                Span::raw(format!("{}", addr)),
            ]));
        } else if let Some(addr) = state.current_peer {
            lines.push(Line::from(vec![
                Span::styled("Connecting to: ", Style::default().fg(Color::Yellow)),
                Span::raw(format!("{}", addr)),
            ]));
        }

        if let Some(ref error) = state.error_message {
            lines.push(Line::from(vec![
                Span::styled("Error: ", Style::default().fg(Color::Red)),
                Span::raw(error),
            ]));
        }

        let paragraph =
            Paragraph::new(lines).block(Block::default().title("Connection").borders(Borders::ALL));
        f.render_widget(paragraph, area);
    }

    fn draw_progress(f: &mut Frame, area: Rect, state: &UIState) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Progress bar
                Constraint::Length(5), // Stats and current piece info (increased from 3)
            ])
            .split(area);

        // Progress bar
        let progress = state.progress_percentage();
        let gauge = Gauge::default()
            .block(
                Block::default()
                    .title("Download Progress")
                    .borders(Borders::ALL),
            )
            .gauge_style(Style::default().fg(Color::Green))
            .percent(progress as u16)
            .label(format!("{:.1}%", progress));
        f.render_widget(gauge, chunks[0]);

        // Current piece and stats
        if state.download_started {
            let mut lines = Vec::new();

            // Current piece info
            if let Some(current_piece) = state.current_piece_index {
                lines.push(Line::from(vec![
                    Span::styled("Current Piece: ", Style::default().fg(Color::Yellow)),
                    Span::raw(format!("{}", current_piece + 1)),
                    Span::styled(" / ", Style::default().fg(Color::Gray)),
                    Span::raw(format!("{}", state.total_pieces)),
                    Span::styled("    Downloaded: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}/{}", state.completed_pieces, state.total_pieces)),
                ]));
            }

            // Speed line - show even if speeds are 0 for debugging
            let mut speed_line = Vec::new();
            speed_line.push(Span::styled("Speed: ", Style::default().fg(Color::Green)));

            if state.bytes_per_second > 0.0 {
                speed_line.push(Span::raw(format!(
                    "{}/s",
                    format_bytes(state.bytes_per_second as u64)
                )));

                if state.pieces_per_minute >= 1.0 {
                    speed_line.push(Span::styled("  (", Style::default().fg(Color::Gray)));
                    speed_line.push(Span::raw(format!(
                        "{:.1} pieces/min",
                        state.pieces_per_minute
                    )));
                    speed_line.push(Span::styled(")", Style::default().fg(Color::Gray)));
                } else if state.download_speed > 0.0 {
                    speed_line.push(Span::styled("  (", Style::default().fg(Color::Gray)));
                    speed_line.push(Span::raw(format!("{:.2} pieces/sec", state.download_speed)));
                    speed_line.push(Span::styled(")", Style::default().fg(Color::Gray)));
                }
            } else {
                speed_line.push(Span::styled(
                    "Calculating...",
                    Style::default().fg(Color::Yellow),
                ));
                // Debug info
                if state.completed_pieces > 0 {
                    speed_line.push(Span::styled(" [", Style::default().fg(Color::Gray)));
                    speed_line.push(Span::raw(format!("{}p", state.completed_pieces)));
                    if let Some(start_time) = state.start_time {
                        let elapsed = start_time.elapsed().as_secs();
                        speed_line.push(Span::raw(format!(", {}s", elapsed)));
                    }
                    speed_line.push(Span::styled("]", Style::default().fg(Color::Gray)));
                }
            }
            lines.push(Line::from(speed_line));

            // Timing line - always show if download started
            let mut timing_line = Vec::new();
            if let Some(start_time) = state.start_time {
                let elapsed = start_time.elapsed();
                timing_line.push(Span::styled("Elapsed: ", Style::default().fg(Color::Blue)));
                timing_line.push(Span::raw(format_duration(elapsed)));
            }

            if let Some(eta) = state.eta {
                if !timing_line.is_empty() {
                    timing_line.push(Span::styled("    ", Style::default()));
                }
                timing_line.push(Span::styled("ETA: ", Style::default().fg(Color::Magenta)));
                timing_line.push(Span::raw(format_duration(eta)));
            } else if state.completed_pieces > 0 {
                if !timing_line.is_empty() {
                    timing_line.push(Span::styled("    ", Style::default()));
                }
                timing_line.push(Span::styled("ETA: ", Style::default().fg(Color::Gray)));
                timing_line.push(Span::styled(
                    "Calculating...",
                    Style::default().fg(Color::Yellow),
                ));
            }

            if !timing_line.is_empty() {
                lines.push(Line::from(timing_line));
            }

            let stats_paragraph = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Download Status"),
            );
            f.render_widget(stats_paragraph, chunks[1]);
        }
    }

    fn draw_logs(f: &mut Frame, area: Rect, state: &UIState) {
        let logs: Vec<ListItem> = state
            .log_messages
            .iter()
            .rev() // Show newest first
            .take(area.height.saturating_sub(2) as usize)
            .map(|msg| ListItem::new(msg.as_str()))
            .collect();

        let logs_list =
            List::new(logs).block(Block::default().title("Activity Log").borders(Borders::ALL));
        f.render_widget(logs_list, area);
    }

    fn restore_terminal(&mut self) -> Result<(), io::Error> {
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for UI {
    fn drop(&mut self) {
        let _ = self.restore_terminal();
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}
