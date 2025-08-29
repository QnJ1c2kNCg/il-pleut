use crate::download::Downloader;
use crate::peer_manager::PeerClient;
use crate::wire::PeerMessage;
use crate::{
    parser::{TorrentFiles, parse_torrent_file},
    tracker::TrackerClient,
};
use std::net::SocketAddr;

mod download;
mod parser;
mod peer_manager;
mod tracker;
mod wire;

#[tokio::main]
async fn main() {
    // Example of parsing a torrent file
    let torrent = parse_torrent_file("samples/ubuntu-25.04-desktop-amd64.iso.torrent").unwrap();
    println!("Torrent name: {}", torrent.info.name);
    println!("Announce URL: {}", torrent.announce);
    println!("Info hash: {:?}", torrent.info_hash);
    println!("Piece length: {}", torrent.info.piece_length);
    println!("Number of pieces: {}", torrent.info.pieces.len());

    match &torrent.info.files {
        TorrentFiles::Single { length } => {
            println!("Single file, length: {} bytes", length);
        }
        TorrentFiles::Multiple { files } => {
            println!("Multi-file torrent with {} files:", files.len());
            for file in files {
                println!("  {}: {} bytes", file.path.join("/"), file.length);
            }
        }
    }

    println!("Done parsing torrent file.\n");

    // Create tracker client
    let tracker_client = TrackerClient::new();

    // Announce to tracker
    let response = tracker_client.announce(&torrent).await.unwrap();

    println!("Tracker response:");
    println!("  Interval: {} seconds", response.interval);
    println!("  Seeders: {}", response.complete);
    println!("  Leechers: {}", response.incomplete);
    println!("  Peers found: {}", response.peers.len());

    for (i, peer) in response.peers.iter().enumerate().take(5) {
        println!("  Peer {}: {}:{}", i + 1, peer.ip, peer.port);
    }

    if let Some(warning) = response.warning_message {
        println!("  Warning: {}", warning);
    }

    // Find a suitable peer and start downloading
    let mut connected = false;
    for peer in &response.peers {
        let addr = SocketAddr::new(peer.ip, peer.port);
        println!("\nConnecting to peer: {}", addr);
        match PeerClient::connect(
            addr,
            torrent.info_hash,
            tracker_client.get_peer_id().clone(),
        ) {
            Ok(mut peer_client) => {
                println!("Handshake successful with peer: {:?}", peer_client.addr);

                // Create downloader and start downloading
                let output_filename = format!("{}.download", torrent.info.name);
                println!("Downloading to: {}", output_filename);

                match Downloader::new(torrent.clone(), &output_filename) {
                    Ok(mut downloader) => match downloader.download(&mut peer_client) {
                        Ok(()) => {
                            println!("Download completed successfully!");
                            connected = true;
                            break;
                        }
                        Err(e) => {
                            println!("Download failed: {}", e);
                        }
                    },
                    Err(e) => {
                        println!("Failed to create downloader: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("Failed to connect/handshake with peer: {}", e);
                // Try next peer
                continue;
            }
        }
    }

    if !connected {
        println!("Failed to connect to any peers or download failed");
    }
}
