/// Tracker client for announcing to BitTorrent trackers and parsing responses.
use crate::parser::{BencodeParser, BencodeValue, ParseError, TorrentFile};
use rand::Rng;
use reqwest;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

#[derive(Debug)]
pub struct TrackerError {
    pub message: String,
}

impl std::fmt::Display for TrackerError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Tracker error: {}", self.message)
    }
}

impl std::error::Error for TrackerError {}

impl From<reqwest::Error> for TrackerError {
    fn from(err: reqwest::Error) -> Self {
        TrackerError {
            message: format!("HTTP error: {}", err),
        }
    }
}

impl From<ParseError> for TrackerError {
    fn from(err: ParseError) -> Self {
        TrackerError {
            message: format!("Parse error: {}", err),
        }
    }
}

impl From<url::ParseError> for TrackerError {
    fn from(err: url::ParseError) -> Self {
        TrackerError {
            message: format!("URL parse error: {}", err),
        }
    }
}

#[derive(Debug, Clone)]
pub enum TrackerEvent {
    Started,
    Stopped,
    Completed,
    // None is represented by not including the event parameter
}

impl TrackerEvent {
    fn as_str(&self) -> &str {
        match self {
            TrackerEvent::Started => "started",
            TrackerEvent::Stopped => "stopped",
            TrackerEvent::Completed => "completed",
        }
    }
}

#[derive(Debug)]
pub struct TrackerRequest {
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
    pub compact: bool,
    pub no_peer_id: bool,
    pub event: Option<TrackerEvent>,
    pub ip: Option<IpAddr>,
    pub numwant: Option<u32>,
    pub key: Option<u32>,
    pub trackerid: Option<String>,
}

#[derive(Debug)]
pub struct Peer {
    pub ip: IpAddr,
    pub port: u16,
    pub peer_id: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct TrackerResponse {
    pub failure_reason: Option<String>,
    pub warning_message: Option<String>,
    pub interval: u32,
    pub min_interval: Option<u32>,
    pub tracker_id: Option<String>,
    pub complete: u32,   // seeders
    pub incomplete: u32, // leechers
    pub downloaded: Option<u32>,
    pub peers: Vec<Peer>,
}

pub struct TrackerClient {
    client: reqwest::Client,
    peer_id: [u8; 20],
}

impl TrackerClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("qBittorrent/4.5.0") // Mimic popular client
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        let peer_id = Self::generate_peer_id();

        Self { client, peer_id }
    }

    pub fn new_with_peer_id(peer_id: [u8; 20]) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("BitTorrent/1.0")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { client, peer_id }
    }

    fn generate_peer_id() -> [u8; 20] {
        let mut peer_id = [0u8; 20];
        // Use qBittorrent-style peer ID: -qB4500-<12 random bytes>
        peer_id[0] = b'-';
        peer_id[1] = b'q';
        peer_id[2] = b'B';
        peer_id[3] = b'4';
        peer_id[4] = b'5';
        peer_id[5] = b'0';
        peer_id[6] = b'0';
        peer_id[7] = b'-';

        for i in 8..20 {
            peer_id[i] = rand::random::<u8>();
        }

        peer_id
    }

    pub fn get_peer_id(&self) -> &[u8; 20] {
        &self.peer_id
    }

    pub async fn announce(
        &self,
        tracker_url: &str,
        request: &TrackerRequest,
    ) -> Result<TrackerResponse, TrackerError> {
        let mut url = Url::parse(tracker_url)?;

        // Use percent-encoding for info_hash and peer_id as raw bytes
        use percent_encoding::{NON_ALPHANUMERIC, percent_encode};

        let info_hash_encoded = percent_encode(&request.info_hash, NON_ALPHANUMERIC).to_string();
        let peer_id_encoded = percent_encode(&request.peer_id, NON_ALPHANUMERIC).to_string();

        // Build query string manually to avoid double-encoding
        let mut query = format!(
            "info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact={}",
            info_hash_encoded,
            peer_id_encoded,
            request.port,
            request.uploaded,
            request.downloaded,
            request.left,
            if request.compact { "1" } else { "0" }
        );

        if request.no_peer_id {
            query.push_str("&no_peer_id=1");
        }
        if let Some(numwant) = request.numwant {
            query.push_str(&format!("&numwant={}", numwant));
        }

        url.set_query(Some(&query));

        println!("Announcing to tracker: {}", url);

        // Make the request
        let response = self.client.get(url).send().await?;
        let response_bytes = response.bytes().await?;

        // Debug: Print first 200 bytes of response
        println!(
            "Response preview: {:?}",
            String::from_utf8_lossy(&response_bytes[..std::cmp::min(200, response_bytes.len())])
        );

        // Check if response looks like HTML (starts with '<')
        if response_bytes.starts_with(b"<") {
            return Err(TrackerError {
                message: format!(
                    "Tracker returned HTML instead of bencode. Response: {}",
                    String::from_utf8_lossy(
                        &response_bytes[..std::cmp::min(500, response_bytes.len())]
                    )
                ),
            });
        }

        // Parse the bencode response
        let mut parser = BencodeParser::new(&response_bytes);
        let response_value = parser.parse()?;
        let response_dict = response_value.as_dict().map_err(|_| TrackerError {
            message: "Tracker response is not a dictionary".to_string(),
        })?;

        // Check for failure
        if let Some(failure_reason) = response_dict.get(b"failure reason".as_ref()) {
            let reason =
                String::from_utf8_lossy(failure_reason.as_bytes().map_err(|_| TrackerError {
                    message: "Invalid failure reason".to_string(),
                })?);
            return Err(TrackerError {
                message: format!("Tracker failure: {}", reason),
            });
        }

        // Parse response fields
        let warning_message = response_dict
            .get(b"warning message".as_ref())
            .map(|v| String::from_utf8_lossy(v.as_bytes().unwrap_or(b"")).to_string());

        let interval = response_dict
            .get(b"interval".as_ref())
            .ok_or_else(|| TrackerError {
                message: "Missing interval field".to_string(),
            })?
            .as_integer()
            .map_err(|_| TrackerError {
                message: "Invalid interval".to_string(),
            })? as u32;

        let min_interval = response_dict
            .get(b"min interval".as_ref())
            .map(|v| v.as_integer().unwrap_or(0) as u32);

        let tracker_id = response_dict
            .get(b"tracker id".as_ref())
            .map(|v| String::from_utf8_lossy(v.as_bytes().unwrap_or(b"")).to_string());

        let complete = response_dict
            .get(b"complete".as_ref())
            .map(|v| v.as_integer().unwrap_or(0) as u32)
            .unwrap_or(0);

        let incomplete = response_dict
            .get(b"incomplete".as_ref())
            .map(|v| v.as_integer().unwrap_or(0) as u32)
            .unwrap_or(0);

        let downloaded = response_dict
            .get(b"downloaded".as_ref())
            .map(|v| v.as_integer().unwrap_or(0) as u32);

        // Parse peers
        let peers = if let Some(peers_value) = response_dict.get(b"peers".as_ref()) {
            if request.compact {
                Self::parse_compact_peers(peers_value)?
            } else {
                Self::parse_dict_peers(peers_value)?
            }
        } else {
            Vec::new()
        };

        Ok(TrackerResponse {
            failure_reason: None,
            warning_message,
            interval,
            min_interval,
            tracker_id,
            complete,
            incomplete,
            downloaded,
            peers,
        })
    }

    fn parse_compact_peers(peers_value: &BencodeValue) -> Result<Vec<Peer>, TrackerError> {
        let peers_bytes = peers_value.as_bytes().map_err(|_| TrackerError {
            message: "Compact peers must be bytes".to_string(),
        })?;

        if peers_bytes.len() % 6 != 0 {
            return Err(TrackerError {
                message: "Invalid compact peers length".to_string(),
            });
        }

        let mut peers = Vec::new();
        for chunk in peers_bytes.chunks(6) {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);

            peers.push(Peer {
                ip: IpAddr::V4(ip),
                port,
                peer_id: None,
            });
        }

        Ok(peers)
    }

    fn parse_dict_peers(peers_value: &BencodeValue) -> Result<Vec<Peer>, TrackerError> {
        let peers_list = peers_value.as_list().map_err(|_| TrackerError {
            message: "Non-compact peers must be a list".to_string(),
        })?;

        let mut peers = Vec::new();
        for peer_value in peers_list {
            let peer_dict = peer_value.as_dict().map_err(|_| TrackerError {
                message: "Peer must be a dictionary".to_string(),
            })?;

            let ip_str = String::from_utf8_lossy(
                peer_dict
                    .get(b"ip".as_ref())
                    .ok_or_else(|| TrackerError {
                        message: "Missing peer IP".to_string(),
                    })?
                    .as_bytes()
                    .map_err(|_| TrackerError {
                        message: "Invalid peer IP".to_string(),
                    })?,
            );

            let ip = ip_str.parse::<IpAddr>().map_err(|_| TrackerError {
                message: "Invalid IP address".to_string(),
            })?;

            let port = peer_dict
                .get(b"port".as_ref())
                .ok_or_else(|| TrackerError {
                    message: "Missing peer port".to_string(),
                })?
                .as_integer()
                .map_err(|_| TrackerError {
                    message: "Invalid peer port".to_string(),
                })? as u16;

            let peer_id = peer_dict
                .get(b"peer id".as_ref())
                .map(|v| v.as_bytes().unwrap_or(b"").to_vec());

            peers.push(Peer { ip, port, peer_id });
        }

        Ok(peers)
    }

    fn url_encode_bytes(bytes: &[u8]) -> String {
        let mut result = String::new();
        for &byte in bytes {
            match byte {
                // Unreserved characters - safe to include as-is
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(byte as char);
                }
                // Everything else gets percent-encoded
                _ => {
                    result.push_str(&format!("%{:02X}", byte));
                }
            }
        }
        result
    }

    /// Create a tracker request for starting a download
    pub fn create_start_request(
        &self,
        torrent: &TorrentFile,
        port: u16,
        downloaded: u64,
        left: u64,
    ) -> TrackerRequest {
        TrackerRequest {
            info_hash: torrent.info_hash,
            peer_id: self.peer_id,
            port,
            uploaded: 0,
            downloaded,
            left,
            compact: true,
            no_peer_id: false,
            event: Some(TrackerEvent::Started),
            ip: None,
            numwant: Some(50),                // Request up to 50 peers
            key: Some(rand::random::<u32>()), // Random key for identification
            trackerid: None,
        }
    }

    /// Create a tracker request for periodic updates
    pub fn create_update_request(
        &self,
        torrent: &TorrentFile,
        port: u16,
        uploaded: u64,
        downloaded: u64,
        left: u64,
        tracker_id: Option<String>,
    ) -> TrackerRequest {
        TrackerRequest {
            info_hash: torrent.info_hash,
            peer_id: self.peer_id,
            port,
            uploaded,
            downloaded,
            left,
            compact: true,
            no_peer_id: false,
            event: None, // No event for regular updates
            ip: None,
            numwant: Some(50),
            key: Some(rand::random::<u32>()),
            trackerid: tracker_id,
        }
    }

    /// Create a minimal tracker request for testing
    pub fn create_minimal_request(&self, torrent: &TorrentFile, port: u16) -> TrackerRequest {
        TrackerRequest {
            info_hash: torrent.info_hash,
            peer_id: self.peer_id,
            port,
            uploaded: 0,
            downloaded: 0,
            left: torrent.total_size(),
            compact: true,
            no_peer_id: false,
            event: None, // No event
            ip: None,
            numwant: None, // No numwant
            key: None,     // No key
            trackerid: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_id_generation() {
        let client = TrackerClient::new();
        let peer_id = client.get_peer_id();

        // Should start with -RS0001-
        assert_eq!(&peer_id[0..8], b"-RS0001-");
        assert_eq!(peer_id.len(), 20);
    }

    #[test]
    fn test_url_encoding() {
        let bytes = b"Hello World!";
        let encoded = TrackerClient::url_encode_bytes(bytes);
        assert_eq!(encoded, "Hello%20World%21");
    }

    #[test]
    fn test_compact_peer_parsing() {
        // Example: IP 192.168.1.1, port 6881 (0x1AE1)
        let peer_bytes = vec![192, 168, 1, 1, 0x1A, 0xE1];
        let bencode_value = BencodeValue::String(peer_bytes);

        let peers = TrackerClient::parse_compact_peers(&bencode_value).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].ip, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(peers[0].port, 6881);
    }
}
