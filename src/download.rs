use crate::parser::{TorrentFile, TorrentFiles};
use crate::peer_manager::PeerClient;
use crate::ui::UIEvent;
use crate::wire::PeerMessage;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

const BLOCK_SIZE: u32 = 16384; // 16KB standard block size

#[derive(Debug)]
pub struct DownloadError {
    pub message: String,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Download error: {}", self.message)
    }
}

impl std::error::Error for DownloadError {}

impl From<io::Error> for DownloadError {
    fn from(err: io::Error) -> Self {
        DownloadError {
            message: format!("IO error: {}", err),
        }
    }
}

#[derive(Debug, Clone)]
struct Block {
    index: u32,
    begin: u32,
    data: Vec<u8>,
}

#[derive(Debug)]
struct PieceBuffer {
    blocks: HashMap<u32, Block>, // key is begin offset
    total_size: u32,
    received_size: u32,
}

impl PieceBuffer {
    fn new(piece_size: u32) -> Self {
        PieceBuffer {
            blocks: HashMap::new(),
            total_size: piece_size,
            received_size: 0,
        }
    }

    fn add_block(&mut self, block: Block) -> bool {
        if !self.blocks.contains_key(&block.begin) {
            self.received_size += block.data.len() as u32;
            self.blocks.insert(block.begin, block);
        }
        self.is_complete()
    }

    fn is_complete(&self) -> bool {
        self.received_size >= self.total_size
    }

    fn assemble(&self) -> Vec<u8> {
        let mut piece_data = vec![0u8; self.total_size as usize];

        for block in self.blocks.values() {
            let start = block.begin as usize;
            let end = start + block.data.len();
            if end <= piece_data.len() {
                piece_data[start..end].copy_from_slice(&block.data);
            }
        }

        piece_data
    }
}

pub struct Downloader {
    torrent: TorrentFile,
    output_file: File,
    completed_pieces: Vec<bool>,
    current_piece_buffer: Option<(u32, PieceBuffer)>,
    peer_bitfield: Option<Vec<u8>>,
    peer_choked: bool,
    ui_sender: Option<Sender<UIEvent>>,
    stop_signal: Option<Arc<AtomicBool>>,
}

impl Downloader {
    pub fn new(torrent: TorrentFile, output_path: &str) -> Result<Self, DownloadError> {
        // Create or truncate the output file
        let output_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(output_path)?;

        // Pre-allocate file size for single file torrents
        if let TorrentFiles::Single { length } = &torrent.info.files {
            output_file.set_len(*length)?;
        }

        let num_pieces = torrent.info.pieces.len();

        Ok(Downloader {
            torrent,
            output_file,
            completed_pieces: vec![false; num_pieces],
            current_piece_buffer: None,
            peer_bitfield: None,
            peer_choked: true,
            ui_sender: None,
            stop_signal: None,
        })
    }

    pub fn with_ui_sender(mut self, sender: Sender<UIEvent>) -> Self {
        self.ui_sender = Some(sender);
        self
    }

    pub fn with_stop_signal(mut self, stop_signal: Arc<AtomicBool>) -> Self {
        self.stop_signal = Some(stop_signal);
        self
    }

    pub fn download(&mut self, peer: &mut PeerClient) -> Result<(), DownloadError> {
        if let Some(ref sender) = self.ui_sender {
            let _ = sender.send(UIEvent::DownloadStarted);
        }

        // Wait for bitfield and initial messages
        self.handle_initial_messages(peer)?;

        if self.peer_choked {
            return Err(DownloadError {
                message: "Peer never unchoked us".to_string(),
            });
        }

        // Download pieces in order
        for piece_index in 0..self.torrent.info.pieces.len() {
            // Check if we should stop
            if let Some(ref stop_signal) = self.stop_signal {
                if stop_signal.load(Ordering::Relaxed) {
                    return Err(DownloadError {
                        message: "Download stopped by user".to_string(),
                    });
                }
            }

            if self.can_download_piece(piece_index as u32) {
                self.download_piece(peer, piece_index as u32)?;
            } else {
                return Err(DownloadError {
                    message: format!("Peer doesn't have piece {}", piece_index),
                });
            }
        }

        if let Some(ref sender) = self.ui_sender {
            let _ = sender.send(UIEvent::DownloadComplete);
        }
        Ok(())
    }

    fn handle_initial_messages(&mut self, peer: &mut PeerClient) -> Result<(), DownloadError> {
        // Send interested message
        peer.send_message(&PeerMessage::Interested)
            .map_err(|e| DownloadError {
                message: format!("Failed to send interested: {}", e),
            })?;

        // Handle initial messages
        let mut messages_received = 0;
        while messages_received < 10 {
            // Limit to avoid infinite loop
            match peer.receive_message() {
                Ok(msg) => {
                    messages_received += 1;
                    match msg {
                        PeerMessage::Bitfield(bits) => {
                            self.peer_bitfield = Some(bits);
                        }
                        PeerMessage::Unchoke => {
                            self.peer_choked = false;
                            break; // Ready to start downloading
                        }
                        PeerMessage::Choke => {
                            self.peer_choked = true;
                        }
                        PeerMessage::Have(piece_index) => {
                            self.update_peer_has_piece(piece_index);
                        }
                        PeerMessage::KeepAlive => {
                            // Ignore keep-alive messages
                        }
                        _other => {
                            // Ignore other messages during initialization
                        }
                    }
                }
                Err(e) => {
                    return Err(DownloadError {
                        message: format!("Failed to receive message: {}", e),
                    });
                }
            }
        }

        Ok(())
    }

    fn can_download_piece(&self, piece_index: u32) -> bool {
        if let Some(ref bitfield) = self.peer_bitfield {
            let byte_index = (piece_index / 8) as usize;
            let bit_index = 7 - (piece_index % 8);

            if byte_index < bitfield.len() {
                return (bitfield[byte_index] >> bit_index) & 1 == 1;
            }
        }
        false
    }

    fn update_peer_has_piece(&mut self, piece_index: u32) {
        if let Some(ref mut bitfield) = self.peer_bitfield {
            let byte_index = (piece_index / 8) as usize;
            let bit_index = 7 - (piece_index % 8);

            if byte_index < bitfield.len() {
                bitfield[byte_index] |= 1 << bit_index;
            }
        }
    }

    fn download_piece(
        &mut self,
        peer: &mut PeerClient,
        piece_index: u32,
    ) -> Result<(), DownloadError> {
        let piece_size = self.get_piece_size(piece_index);
        let mut piece_buffer = PieceBuffer::new(piece_size);

        // Request all blocks for this piece
        let num_blocks = (piece_size + BLOCK_SIZE - 1) / BLOCK_SIZE; // Ceiling division
        for block_index in 0..num_blocks {
            let begin = block_index * BLOCK_SIZE;
            let length = std::cmp::min(BLOCK_SIZE, piece_size - begin);

            let request = PeerMessage::Request {
                index: piece_index,
                begin,
                length,
            };

            peer.send_message(&request).map_err(|e| DownloadError {
                message: format!("Failed to send request: {}", e),
            })?;
        }

        // Receive blocks until piece is complete
        let mut blocks_received = 0;
        while !piece_buffer.is_complete() && blocks_received < num_blocks * 2 {
            // Check if we should stop
            if let Some(ref stop_signal) = self.stop_signal {
                if stop_signal.load(Ordering::Relaxed) {
                    return Err(DownloadError {
                        message: "Download stopped by user".to_string(),
                    });
                }
            }

            match peer.receive_message() {
                Ok(PeerMessage::Piece {
                    index,
                    begin,
                    block,
                }) => {
                    if index == piece_index {
                        let block = Block {
                            index,
                            begin,
                            data: block,
                        };

                        if piece_buffer.add_block(block) {
                            break; // Piece is complete
                        }
                    }
                    blocks_received += 1;
                }
                Ok(PeerMessage::Choke) => {
                    return Err(DownloadError {
                        message: "Peer choked us during download".to_string(),
                    });
                }
                Ok(PeerMessage::KeepAlive) => {
                    // Ignore keep-alive
                }
                Ok(_other) => {
                    // Ignore other messages during piece download
                }
                Err(e) => {
                    return Err(DownloadError {
                        message: format!("Failed to receive piece data: {}", e),
                    });
                }
            }
        }

        if !piece_buffer.is_complete() {
            return Err(DownloadError {
                message: format!("Failed to download complete piece {}", piece_index),
            });
        }

        // Verify and write piece
        let piece_data = piece_buffer.assemble();
        self.verify_and_write_piece(piece_index, piece_data)?;

        Ok(())
    }

    fn get_piece_size(&self, piece_index: u32) -> u32 {
        let total_size = self.torrent.total_size();
        let piece_length = self.torrent.info.piece_length;

        if piece_index == (self.torrent.info.pieces.len() - 1) as u32 {
            // Last piece might be smaller
            let remaining = total_size % piece_length as u64;
            if remaining == 0 {
                piece_length
            } else {
                remaining as u32
            }
        } else {
            piece_length
        }
    }

    fn verify_and_write_piece(
        &mut self,
        piece_index: u32,
        data: Vec<u8>,
    ) -> Result<(), DownloadError> {
        // Verify SHA-1 hash
        let mut hasher = Sha1::new();
        hasher.update(&data);
        let hash: [u8; 20] = hasher.finalize().into();

        let expected_hash = &self.torrent.info.pieces[piece_index as usize];

        if hash != *expected_hash {
            return Err(DownloadError {
                message: format!("Piece {} failed hash verification", piece_index),
            });
        }

        // Write to file at correct offset
        let offset = piece_index as u64 * self.torrent.info.piece_length as u64;
        self.output_file.seek(SeekFrom::Start(offset))?;
        self.output_file.write_all(&data)?;
        self.output_file.flush()?;

        // Mark piece as completed
        self.completed_pieces[piece_index as usize] = true;

        let completed = self.completed_pieces.iter().filter(|&&x| x).count();
        let total = self.completed_pieces.len();

        // Send progress update to UI
        if let Some(ref sender) = self.ui_sender {
            let _ = sender.send(UIEvent::PieceCompleted(piece_index, completed, total));
        }

        Ok(())
    }

    pub fn get_progress(&self) -> (usize, usize) {
        let completed = self.completed_pieces.iter().filter(|&&x| x).count();
        let total = self.completed_pieces.len();
        (completed, total)
    }
}
