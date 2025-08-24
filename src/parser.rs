/// Parses the `.torrent` file and returns a Torrent struct.
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Clone, PartialEq)]
pub enum BencodeValue {
    String(Vec<u8>),
    Integer(i64),
    List(Vec<BencodeValue>),
    Dictionary(HashMap<Vec<u8>, BencodeValue>),
}

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Parse error: {}", self.message)
    }
}

impl std::error::Error for ParseError {}

pub struct BencodeParser<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> BencodeParser<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub fn parse(&mut self) -> Result<BencodeValue, ParseError> {
        if self.position >= self.data.len() {
            return Err(ParseError {
                message: "Unexpected end of data".to_string(),
            });
        }

        match self.data[self.position] {
            b'i' => self.parse_integer(),
            b'l' => self.parse_list(),
            b'd' => self.parse_dictionary(),
            b'0'..=b'9' => self.parse_string(),
            _ => Err(ParseError {
                message: format!("Unexpected character: {}", self.data[self.position] as char),
            }),
        }
    }

    fn parse_integer(&mut self) -> Result<BencodeValue, ParseError> {
        self.position += 1; // skip 'i'
        let start = self.position;

        while self.position < self.data.len() && self.data[self.position] != b'e' {
            self.position += 1;
        }

        if self.position >= self.data.len() {
            return Err(ParseError {
                message: "Unterminated integer".to_string(),
            });
        }

        let int_str =
            std::str::from_utf8(&self.data[start..self.position]).map_err(|_| ParseError {
                message: "Invalid UTF-8 in integer".to_string(),
            })?;

        let value = int_str.parse::<i64>().map_err(|_| ParseError {
            message: "Invalid integer".to_string(),
        })?;

        self.position += 1; // skip 'e'
        Ok(BencodeValue::Integer(value))
    }

    fn parse_string(&mut self) -> Result<BencodeValue, ParseError> {
        let start = self.position;

        while self.position < self.data.len() && self.data[self.position] != b':' {
            self.position += 1;
        }

        if self.position >= self.data.len() {
            return Err(ParseError {
                message: "Unterminated string length".to_string(),
            });
        }

        let length_str =
            std::str::from_utf8(&self.data[start..self.position]).map_err(|_| ParseError {
                message: "Invalid UTF-8 in string length".to_string(),
            })?;

        let length = length_str.parse::<usize>().map_err(|_| ParseError {
            message: "Invalid string length".to_string(),
        })?;

        self.position += 1; // skip ':'

        if self.position + length > self.data.len() {
            return Err(ParseError {
                message: "String longer than remaining data".to_string(),
            });
        }

        let string_data = self.data[self.position..self.position + length].to_vec();
        self.position += length;

        Ok(BencodeValue::String(string_data))
    }

    fn parse_list(&mut self) -> Result<BencodeValue, ParseError> {
        self.position += 1; // skip 'l'
        let mut list = Vec::new();

        while self.position < self.data.len() && self.data[self.position] != b'e' {
            list.push(self.parse()?);
        }

        if self.position >= self.data.len() {
            return Err(ParseError {
                message: "Unterminated list".to_string(),
            });
        }

        self.position += 1; // skip 'e'
        Ok(BencodeValue::List(list))
    }

    fn parse_dictionary(&mut self) -> Result<BencodeValue, ParseError> {
        self.position += 1; // skip 'd'
        let mut dict = HashMap::new();

        while self.position < self.data.len() && self.data[self.position] != b'e' {
            let key = match self.parse()? {
                BencodeValue::String(s) => s,
                _ => {
                    return Err(ParseError {
                        message: "Dictionary key must be a string".to_string(),
                    });
                }
            };

            let value = self.parse()?;
            dict.insert(key, value);
        }

        if self.position >= self.data.len() {
            return Err(ParseError {
                message: "Unterminated dictionary".to_string(),
            });
        }

        self.position += 1; // skip 'e'
        Ok(BencodeValue::Dictionary(dict))
    }
}

#[derive(Debug)]
pub struct TorrentFile {
    pub announce: String,
    pub announce_list: Option<Vec<Vec<String>>>,
    pub info: TorrentInfo,
    pub info_hash: [u8; 20],
}

impl TorrentFile {
    /// Calculate the total size of all files in the torrent
    pub fn total_size(&self) -> u64 {
        match &self.info.files {
            TorrentFiles::Single { length } => *length,
            TorrentFiles::Multiple { files } => files.iter().map(|f| f.length).sum(),
        }
    }
}

#[derive(Debug)]
pub struct TorrentInfo {
    pub name: String,
    pub piece_length: u32,
    pub pieces: Vec<[u8; 20]>,
    pub files: TorrentFiles,
}

#[derive(Debug)]
pub enum TorrentFiles {
    Single { length: u64 },
    Multiple { files: Vec<TorrentFileInfo> },
}

#[derive(Debug)]
pub struct TorrentFileInfo {
    pub path: Vec<String>,
    pub length: u64,
}

impl BencodeValue {
    pub(crate) fn as_string(&self) -> Result<String, ParseError> {
        match self {
            BencodeValue::String(bytes) => {
                String::from_utf8(bytes.clone()).map_err(|_| ParseError {
                    message: "Invalid UTF-8 in string".to_string(),
                })
            }
            _ => Err(ParseError {
                message: "Expected string".to_string(),
            }),
        }
    }

    pub(crate) fn as_bytes(&self) -> Result<&[u8], ParseError> {
        match self {
            BencodeValue::String(bytes) => Ok(bytes),
            _ => Err(ParseError {
                message: "Expected string/bytes".to_string(),
            }),
        }
    }

    pub(crate) fn as_integer(&self) -> Result<i64, ParseError> {
        match self {
            BencodeValue::Integer(i) => Ok(*i),
            _ => Err(ParseError {
                message: "Expected integer".to_string(),
            }),
        }
    }

    pub(crate) fn as_list(&self) -> Result<&[BencodeValue], ParseError> {
        match self {
            BencodeValue::List(list) => Ok(list),
            _ => Err(ParseError {
                message: "Expected list".to_string(),
            }),
        }
    }

    pub(crate) fn as_dict(&self) -> Result<&HashMap<Vec<u8>, BencodeValue>, ParseError> {
        match self {
            BencodeValue::Dictionary(dict) => Ok(dict),
            _ => Err(ParseError {
                message: "Expected dictionary".to_string(),
            }),
        }
    }
}

pub fn bencode_encode(value: &BencodeValue) -> Vec<u8> {
    match value {
        BencodeValue::String(bytes) => {
            let mut result = bytes.len().to_string().into_bytes();
            result.push(b':');
            result.extend_from_slice(bytes);
            result
        }
        BencodeValue::Integer(i) => {
            let mut result = b"i".to_vec();
            result.extend_from_slice(i.to_string().as_bytes());
            result.push(b'e');
            result
        }
        BencodeValue::List(list) => {
            let mut result = b"l".to_vec();
            for item in list {
                result.extend_from_slice(&bencode_encode(item));
            }
            result.push(b'e');
            result
        }
        BencodeValue::Dictionary(dict) => {
            let mut result = b"d".to_vec();
            let mut keys: Vec<&Vec<u8>> = dict.keys().collect();
            keys.sort();
            for key in keys {
                result.extend_from_slice(&bencode_encode(&BencodeValue::String(key.clone())));
                result.extend_from_slice(&bencode_encode(&dict[key]));
            }
            result.push(b'e');
            result
        }
    }
}

pub fn parse_torrent_file(filename: &str) -> Result<TorrentFile, ParseError> {
    let data = fs::read(filename).map_err(|e| ParseError {
        message: format!("Failed to read file: {}", e),
    })?;

    let mut parser = BencodeParser::new(&data);
    let root = parser.parse()?;
    let root_dict = root.as_dict()?;

    // Extract announce
    let announce = root_dict
        .get(b"announce".as_ref())
        .ok_or_else(|| ParseError {
            message: "Missing 'announce' field".to_string(),
        })?
        .as_string()?;

    // Extract announce-list (optional)
    let announce_list = root_dict
        .get(b"announce-list".as_ref())
        .map(|v| {
            let list = v.as_list()?;
            let mut announce_list = Vec::new();
            for tier in list {
                let tier_list = tier.as_list()?;
                let mut tier_urls = Vec::new();
                for url in tier_list {
                    tier_urls.push(url.as_string()?);
                }
                announce_list.push(tier_urls);
            }
            Ok::<Vec<Vec<String>>, ParseError>(announce_list)
        })
        .transpose()?;

    // Extract info dictionary
    let info_value = root_dict.get(b"info".as_ref()).ok_or_else(|| ParseError {
        message: "Missing 'info' field".to_string(),
    })?;

    let info_dict = info_value.as_dict()?;

    // Calculate info hash
    let info_encoded = bencode_encode(info_value);
    let mut hasher = Sha1::new();
    hasher.update(&info_encoded);
    let info_hash: [u8; 20] = hasher.finalize().into();

    // Parse info dictionary
    let name = info_dict
        .get(b"name".as_ref())
        .ok_or_else(|| ParseError {
            message: "Missing 'name' field in info".to_string(),
        })?
        .as_string()?;

    let piece_length = info_dict
        .get(b"piece length".as_ref())
        .ok_or_else(|| ParseError {
            message: "Missing 'piece length' field".to_string(),
        })?
        .as_integer()? as u32;

    let pieces_bytes = info_dict
        .get(b"pieces".as_ref())
        .ok_or_else(|| ParseError {
            message: "Missing 'pieces' field".to_string(),
        })?
        .as_bytes()?;

    if pieces_bytes.len() % 20 != 0 {
        return Err(ParseError {
            message: "Invalid pieces length (must be multiple of 20)".to_string(),
        });
    }

    let mut pieces = Vec::new();
    for chunk in pieces_bytes.chunks(20) {
        let mut piece_hash = [0u8; 20];
        piece_hash.copy_from_slice(chunk);
        pieces.push(piece_hash);
    }

    // Determine if single or multi-file torrent
    let files = if let Some(length_value) = info_dict.get(b"length".as_ref()) {
        // Single file
        let length = length_value.as_integer()? as u64;
        TorrentFiles::Single { length }
    } else if let Some(files_value) = info_dict.get(b"files".as_ref()) {
        // Multi-file
        let files_list = files_value.as_list()?;
        let mut files = Vec::new();

        for file_value in files_list {
            let file_dict = file_value.as_dict()?;

            let length = file_dict
                .get(b"length".as_ref())
                .ok_or_else(|| ParseError {
                    message: "Missing 'length' in file".to_string(),
                })?
                .as_integer()? as u64;

            let path_list = file_dict
                .get(b"path".as_ref())
                .ok_or_else(|| ParseError {
                    message: "Missing 'path' in file".to_string(),
                })?
                .as_list()?;

            let mut path = Vec::new();
            for path_component in path_list {
                path.push(path_component.as_string()?);
            }

            files.push(TorrentFileInfo { path, length });
        }

        TorrentFiles::Multiple { files }
    } else {
        return Err(ParseError {
            message: "Torrent must have either 'length' or 'files' field".to_string(),
        });
    };

    let info = TorrentInfo {
        name,
        piece_length,
        pieces,
        files,
    };

    Ok(TorrentFile {
        announce,
        announce_list,
        info,
        info_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bencode_string() {
        let mut parser = BencodeParser::new(b"4:spam");
        let result = parser.parse().unwrap();
        assert_eq!(result, BencodeValue::String(b"spam".to_vec()));
    }

    #[test]
    fn test_bencode_integer() {
        let mut parser = BencodeParser::new(b"i42e");
        let result = parser.parse().unwrap();
        assert_eq!(result, BencodeValue::Integer(42));
    }

    #[test]
    fn test_bencode_list() {
        let mut parser = BencodeParser::new(b"l4:spami42ee");
        let result = parser.parse().unwrap();
        assert_eq!(
            result,
            BencodeValue::List(vec![
                BencodeValue::String(b"spam".to_vec()),
                BencodeValue::Integer(42)
            ])
        );
    }

    #[test]
    fn test_bencode_dict() {
        let mut parser = BencodeParser::new(b"d4:spami42ee");
        let result = parser.parse().unwrap();
        let mut expected = HashMap::new();
        expected.insert(b"spam".to_vec(), BencodeValue::Integer(42));
        assert_eq!(result, BencodeValue::Dictionary(expected));
    }
}
