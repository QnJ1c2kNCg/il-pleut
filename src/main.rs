use crate::download::Downloader;
use crate::peer_manager::PeerClient;
use crate::ui::{UI, UIEvent};
use crate::{parser::parse_torrent_file, tracker::TrackerClient};
use clap::Parser;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

mod download;
mod parser;
mod peer_manager;
mod tracker;
mod ui;
mod wire;

/// Il Pleut - A minimal BitTorrent client
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the torrent file to download
    torrent_file: String,

    /// Output directory for downloaded files
    #[arg(short, long, default_value = ".")]
    output: String,

    /// Port to listen on for peer connections
    #[arg(short, long, default_value = "6881")]
    port: u16,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Validate torrent file exists
    if !std::path::Path::new(&args.torrent_file).exists() {
        eprintln!("Error: Torrent file '{}' not found", args.torrent_file);
        std::process::exit(1);
    }

    // Validate output directory
    if args.output != "." {
        if let Err(e) = std::fs::create_dir_all(&args.output) {
            eprintln!(
                "Error: Cannot create output directory '{}': {}",
                args.output, e
            );
            std::process::exit(1);
        }
    }

    // Create UI
    let mut ui = match UI::new() {
        Ok(ui) => ui,
        Err(e) => {
            eprintln!("Failed to create UI: {}", e);
            std::process::exit(1);
        }
    };

    let ui_sender = ui.get_event_sender();
    let should_stop = Arc::new(AtomicBool::new(false));

    // Start download process in background thread
    let download_sender = ui_sender.clone();
    let stop_signal = should_stop.clone();
    let torrent_path = args.torrent_file.clone();
    let output_dir = args.output.clone();
    let port = args.port;
    let download_handle = thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                run_download(download_sender, stop_signal, torrent_path, output_dir, port).await;
            });
    });

    // Run UI (this blocks until user quits)
    if let Err(e) = ui.run() {
        eprintln!("UI error: {}", e);
    }

    // Signal download thread to stop
    should_stop.store(true, Ordering::Relaxed);

    // Wait for download thread to complete with timeout
    let _join_handle = thread::spawn(move || {
        let _ = download_handle.join();
    });

    // Give it 2 seconds to shut down gracefully
    thread::sleep(Duration::from_secs(2));

    // If still running, the process will exit anyway
    println!("Shutting down...");
}

async fn run_download(
    ui_sender: std::sync::mpsc::Sender<UIEvent>,
    should_stop: Arc<AtomicBool>,
    torrent_path: String,
    output_dir: String,
    port: u16,
) {
    // Parse torrent file
    let torrent = match parse_torrent_file(&torrent_path) {
        Ok(torrent) => {
            let _ = ui_sender.send(UIEvent::TorrentParsed(torrent.clone()));
            torrent
        }
        Err(e) => {
            let _ = ui_sender.send(UIEvent::Error(format!("Failed to parse torrent: {}", e)));
            return;
        }
    };

    // Create tracker client
    let tracker_client = TrackerClient::new();

    // Announce to tracker
    let response = match tracker_client.announce(&torrent).await {
        Ok(response) => {
            let _ = ui_sender.send(UIEvent::TrackerResponse(response.clone()));
            response
        }
        Err(e) => {
            let _ = ui_sender.send(UIEvent::Error(format!("Tracker error: {}", e)));
            return;
        }
    };

    // Find a suitable peer and start downloading
    let mut connected = false;
    for peer in &response.peers {
        // Check if we should stop
        if should_stop.load(Ordering::Relaxed) {
            return;
        }

        let addr = SocketAddr::new(peer.ip, peer.port);
        let _ = ui_sender.send(UIEvent::ConnectingToPeer(addr));

        match PeerClient::connect(
            addr,
            torrent.info_hash,
            tracker_client.get_peer_id().clone(),
        ) {
            Ok(mut peer_client) => {
                let _ = ui_sender.send(UIEvent::PeerConnected(addr));

                // Create downloader and start downloading
                let output_filename = if output_dir == "." {
                    format!("{}.download", torrent.info.name)
                } else {
                    format!("{}/{}.download", output_dir, torrent.info.name)
                };

                match Downloader::new(torrent.clone(), &output_filename) {
                    Ok(downloader) => {
                        let mut downloader = downloader
                            .with_ui_sender(ui_sender.clone())
                            .with_stop_signal(should_stop.clone());
                        match downloader.download(&mut peer_client) {
                            Ok(()) => {
                                connected = true;
                                break;
                            }
                            Err(e) => {
                                let _ = ui_sender
                                    .send(UIEvent::Error(format!("Download failed: {}", e)));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = ui_sender.send(UIEvent::Error(format!(
                            "Failed to create downloader: {}",
                            e
                        )));
                    }
                }
            }
            Err(e) => {
                let _ = ui_sender.send(UIEvent::PeerConnectionFailed(addr, e.to_string()));
                continue;
            }
        }

        // Check again after each connection attempt
        if should_stop.load(Ordering::Relaxed) {
            let _ = ui_sender.send(UIEvent::DownloadStopped);
            return;
        }
    }

    if !connected {
        if should_stop.load(Ordering::Relaxed) {
            let _ = ui_sender.send(UIEvent::DownloadStopped);
        } else {
            let _ = ui_sender.send(UIEvent::Error(
                "Failed to connect to any peers or download failed".to_string(),
            ));
        }
    }
}
