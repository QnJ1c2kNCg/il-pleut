use crate::wire::{
    Handshake, PeerMessage, receive_handshake, receive_message, send_handshake, send_message,
};
use std::io;
use std::net::{SocketAddr, TcpStream};

#[derive(Debug)]
pub struct PeerClient {
    pub addr: SocketAddr,
    pub stream: TcpStream,
    pub peer_id: [u8; 20],
    pub info_hash: [u8; 20],
    // Add more state as needed (choked, bitfield, etc.)
}

impl PeerClient {
    pub fn connect(addr: SocketAddr, info_hash: [u8; 20], peer_id: [u8; 20]) -> io::Result<Self> {
        let mut stream = TcpStream::connect(addr)?;
        let handshake = Handshake::new(info_hash, peer_id);
        send_handshake(&mut stream, &handshake)?;
        let peer_handshake = receive_handshake(&mut stream)?;
        Ok(PeerClient {
            addr,
            stream,
            peer_id: peer_handshake.peer_id,
            info_hash: peer_handshake.info_hash,
        })
    }

    pub fn send_message(&mut self, msg: &PeerMessage) -> io::Result<()> {
        send_message(&mut self.stream, msg)
    }

    pub fn receive_message(&mut self) -> io::Result<PeerMessage> {
        receive_message(&mut self.stream)
    }
}

#[derive(Debug)]
pub struct PeerManager {
    pub peers: Vec<PeerClient>,
}

impl PeerManager {
    pub fn new() -> Self {
        PeerManager { peers: Vec::new() }
    }

    pub fn add_peer(&mut self, peer: PeerClient) {
        self.peers.push(peer);
    }

    // Add more management methods as needed (remove, broadcast, etc.)
}
