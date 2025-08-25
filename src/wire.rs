/// Wire protocol implementation for BitTorrent
use std::io::{self, Read, Write};
use std::net::TcpStream;

const BT_PROTOCOL: &str = "BitTorrent protocol";

#[derive(Debug, Clone)]
pub struct Handshake {
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
}

impl Handshake {
    pub fn new(info_hash: [u8; 20], peer_id: [u8; 20]) -> Self {
        Handshake { info_hash, peer_id }
    }

    pub fn serialize(&self) -> [u8; 68] {
        let mut buf = [0u8; 68];
        buf[0] = 19; // pstrlen
        buf[1..20].copy_from_slice(BT_PROTOCOL.as_bytes());
        buf[20..28].copy_from_slice(&[0u8; 8]); // reserved
        buf[28..48].copy_from_slice(&self.info_hash);
        buf[48..68].copy_from_slice(&self.peer_id);
        buf
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() != 68 || data[0] != 19 {
            return None;
        }
        if &data[1..20] != BT_PROTOCOL.as_bytes() {
            return None;
        }
        let mut info_hash = [0u8; 20];
        info_hash.copy_from_slice(&data[28..48]);
        let mut peer_id = [0u8; 20];
        peer_id.copy_from_slice(&data[48..68]);
        Some(Handshake { info_hash, peer_id })
    }
}

#[derive(Debug, Clone)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        block: Vec<u8>,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    Port(u16),
}

impl PeerMessage {
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            PeerMessage::KeepAlive => vec![0, 0, 0, 0],
            PeerMessage::Choke => vec![0, 0, 0, 1, 0],
            PeerMessage::Unchoke => vec![0, 0, 0, 1, 1],
            PeerMessage::Interested => vec![0, 0, 0, 1, 2],
            PeerMessage::NotInterested => vec![0, 0, 0, 1, 3],
            PeerMessage::Have(idx) => {
                let mut v = vec![0, 0, 0, 5, 4];
                v.extend_from_slice(&idx.to_be_bytes());
                v
            }
            PeerMessage::Bitfield(bits) => {
                let len = (1 + bits.len()) as u32;
                let mut v = Vec::with_capacity(4 + 1 + bits.len());
                v.extend_from_slice(&len.to_be_bytes());
                v.push(5);
                v.extend_from_slice(bits);
                v
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                let mut v = vec![0, 0, 0, 13, 6];
                v.extend_from_slice(&index.to_be_bytes());
                v.extend_from_slice(&begin.to_be_bytes());
                v.extend_from_slice(&length.to_be_bytes());
                v
            }
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => {
                let len = (9 + block.len()) as u32;
                let mut v = Vec::with_capacity(4 + 1 + 8 + block.len());
                v.extend_from_slice(&len.to_be_bytes());
                v.push(7);
                v.extend_from_slice(&index.to_be_bytes());
                v.extend_from_slice(&begin.to_be_bytes());
                v.extend_from_slice(block);
                v
            }
            PeerMessage::Cancel {
                index,
                begin,
                length,
            } => {
                let mut v = vec![0, 0, 0, 13, 8];
                v.extend_from_slice(&index.to_be_bytes());
                v.extend_from_slice(&begin.to_be_bytes());
                v.extend_from_slice(&length.to_be_bytes());
                v
            }
            PeerMessage::Port(port) => {
                let mut v = vec![0, 0, 0, 3, 9];
                v.extend_from_slice(&port.to_be_bytes());
                v
            }
        }
    }
}

pub fn send_handshake(stream: &mut TcpStream, handshake: &Handshake) -> io::Result<()> {
    let buf = handshake.serialize();
    stream.write_all(&buf)
}

pub fn receive_handshake(stream: &mut TcpStream) -> io::Result<Handshake> {
    let mut buf = [0u8; 68];
    stream.read_exact(&mut buf)?;
    Handshake::deserialize(&buf)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid handshake"))
}

pub fn send_message(stream: &mut TcpStream, msg: &PeerMessage) -> io::Result<()> {
    let buf = msg.serialize();
    stream.write_all(&buf)
}

pub fn receive_message(stream: &mut TcpStream) -> io::Result<PeerMessage> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len == 0 {
        return Ok(PeerMessage::KeepAlive);
    }
    let mut msg_buf = vec![0u8; len as usize];
    stream.read_exact(&mut msg_buf)?;
    let id = msg_buf[0];
    match id {
        0 => Ok(PeerMessage::Choke),
        1 => Ok(PeerMessage::Unchoke),
        2 => Ok(PeerMessage::Interested),
        3 => Ok(PeerMessage::NotInterested),
        4 => {
            if msg_buf.len() < 5 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid have message",
                ));
            }
            let idx = u32::from_be_bytes([msg_buf[1], msg_buf[2], msg_buf[3], msg_buf[4]]);
            Ok(PeerMessage::Have(idx))
        }
        5 => Ok(PeerMessage::Bitfield(msg_buf[1..].to_vec())),
        6 => {
            if msg_buf.len() < 13 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid request message",
                ));
            }
            let index = u32::from_be_bytes([msg_buf[1], msg_buf[2], msg_buf[3], msg_buf[4]]);
            let begin = u32::from_be_bytes([msg_buf[5], msg_buf[6], msg_buf[7], msg_buf[8]]);
            let length = u32::from_be_bytes([msg_buf[9], msg_buf[10], msg_buf[11], msg_buf[12]]);
            Ok(PeerMessage::Request {
                index,
                begin,
                length,
            })
        }
        7 => {
            if msg_buf.len() < 9 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid piece message",
                ));
            }
            let index = u32::from_be_bytes([msg_buf[1], msg_buf[2], msg_buf[3], msg_buf[4]]);
            let begin = u32::from_be_bytes([msg_buf[5], msg_buf[6], msg_buf[7], msg_buf[8]]);
            let block = msg_buf[9..].to_vec();
            Ok(PeerMessage::Piece {
                index,
                begin,
                block,
            })
        }
        8 => {
            if msg_buf.len() < 13 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid cancel message",
                ));
            }
            let index = u32::from_be_bytes([msg_buf[1], msg_buf[2], msg_buf[3], msg_buf[4]]);
            let begin = u32::from_be_bytes([msg_buf[5], msg_buf[6], msg_buf[7], msg_buf[8]]);
            let length = u32::from_be_bytes([msg_buf[9], msg_buf[10], msg_buf[11], msg_buf[12]]);
            Ok(PeerMessage::Cancel {
                index,
                begin,
                length,
            })
        }
        9 => {
            if msg_buf.len() < 3 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid port message",
                ));
            }
            let port = u16::from_be_bytes([msg_buf[1], msg_buf[2]]);
            Ok(PeerMessage::Port(port))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unknown message id",
        )),
    }
}
