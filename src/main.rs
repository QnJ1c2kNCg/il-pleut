use crate::{
    parser::{TorrentFiles, parse_torrent_file},
    tracker::TrackerClient,
};

mod parser;
mod tracker;

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
    println!("Our peer ID: {:?}", tracker_client.get_peer_id());

    // Create a start request
    // let request = tracker_client.create_start_request(&torrent, 6881, 0, torrent.total_size());
    let request = tracker_client.create_minimal_request(&torrent, 6881);

    // Announce to tracker
    let response = tracker_client
        .announce(&torrent.announce, &request)
        .await
        .unwrap();

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
}
