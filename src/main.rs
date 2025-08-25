use crate::peer_manager::PeerClient;
use crate::wire::PeerMessage;
use crate::{
    parser::{TorrentFiles, parse_torrent_file},
    tracker::TrackerClient,
};
use std::net::SocketAddr;

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

    // Test handshake with the first peer
    if let Some(first_peer) = response.peers.first() {
        let addr = SocketAddr::new(first_peer.ip, first_peer.port);
        println!("\nConnecting to first peer: {}", addr);
        match PeerClient::connect(
            addr,
            torrent.info_hash,
            tracker_client.get_peer_id().clone(),
        ) {
            Ok(mut peer_client) => {
                println!("Handshake successful with peer: {:?}", peer_client.addr);
                // Optionally, send an Interested message and receive a response
                peer_client.send_message(&PeerMessage::Interested).unwrap();
                match peer_client.receive_message() {
                    Ok(msg) => println!(
                        "Received message from peer [{:?}]: {:?}",
                        peer_client.addr, msg
                    ),
                    Err(e) => println!("Error receiving message: {}", e),
                }
            }
            Err(e) => {
                println!("Failed to connect/handshake with peer: {}", e);
            }
        }
    }
}
