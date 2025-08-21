use crate::parser::{TorrentFiles, parse_torrent_file};

mod parser;

fn main() {
    println!("Hello, world!");
    // Example of parsing a torrent file
    let torrent = parse_torrent_file("samples/big-buck-bunny.torrent").unwrap();
    println!("Torrent name: {}", torrent.info.name);
    println!("Announce URL: {}", torrent.announce);
    println!("Info hash: {:?}", torrent.info_hash);
    println!("Piece length: {}", torrent.info.piece_length);
    println!("Number of pieces: {}", torrent.info.pieces.len());

    match torrent.info.files {
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

    println!("Torrent parser implementation complete!");
}
