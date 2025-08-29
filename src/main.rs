use crate::download::Downloader;
use crate::peer_manager::PeerClient;
use crate::ui::{UI, UIEvent};
use crate::{parser::parse_torrent_file, tracker::TrackerClient};
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

#[tokio::main]
async fn main() {
    // Create UI
    let mut ui = match UI::new() {
        Ok(ui) => ui,
        Err(e) => {
            eprintln!("Failed to create UI: {}", e);
            return;
        }
    };

    let ui_sender = ui.get_event_sender();
    let should_stop = Arc::new(AtomicBool::new(false));

    // Start download process in background thread
    let download_sender = ui_sender.clone();
    let stop_signal = should_stop.clone();
    let download_handle = thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                run_download(download_sender, stop_signal).await;
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

async fn run_download(ui_sender: std::sync::mpsc::Sender<UIEvent>, should_stop: Arc<AtomicBool>) {
    // Parse torrent file
    // let torrent = match parse_torrent_file("samples/ubuntu-25.04-desktop-amd64.iso.torrent") {
    let torrent = match parse_torrent_file("samples/archlinux-2025.01.01-x86_64.iso.torrent") {
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
                let output_filename = format!("{}.download", torrent.info.name);

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
