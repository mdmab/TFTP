use std::{
    array::TryFromSliceError,
    fmt,
    io::{self, Error, ErrorKind},
    net::{SocketAddr, UdpSocket},
    str::Utf8Error,
    sync::atomic::fence,
};

/* Constants for opcode. */
pub const OPCODE_RRQ: u16 = 1;
pub const OPCODE_WRQ: u16 = 2;
pub const OPCODE_DATA: u16 = 3;
pub const OPCODE_ACK: u16 = 4;
pub const OPCODE_ERROR: u16 = 5;

/* Default data block size. */
pub const DATA_BLOCK_SIZE: usize = 512;

/* Constants for defining modes. */
pub const MODE_MAIL: &str = "mail";
pub const MODE_NETASCII: &str = "netascii";
pub const MODE_OCTET: &str = "octet";

/* Constants for error codes. */
pub const ERROR_CODE_SEE_MSG: u16 = 0;
pub const ERROR_CODE_FILE_NOT_FOUND: u16 = 1;
pub const ERROR_CODE_ACCESS_VIOLATION: u16 = 2;
pub const ERROR_CODE_DISK_FULL_OR_ALLOC_EXCEEDED: u16 = 3;
pub const ERROR_CODE_ILLEGAL_OP: u16 = 4;
pub const ERROR_CODE_UNKNOWN_TID: u16 = 5;
pub const ERROR_CODE_FILE_EXISTS: u16 = 6;
pub const ERROR_CODE_NO_SUCH_USER: u16 = 7;

/* Default timeout and retry count. */
pub const DEFAULT_TIMEOUT_SEC: u64 = 5;
pub const DEFAULT_RETRY_COUNT: u64 = 5;

/* Most commonly used error messages to be returned when inspecting packet data. */
pub const INVALID_DATA_ERROR: &str = "Invalid data received.";
pub const CORRUPTED_DATA_ERROR: &str = "Received data is corrupted.";
pub const FILENAME_ERROR: &str = "Filename must not contain path or special aliases.";

pub const ETH_FRAME_LEN: usize = 1500;

/* Internal constants. */
const FILENAME_MAXLEN: usize = 128;
const ERROR_MSG_MAXLEN: usize = 256;

/// A struct to hold essential data for each type of TFTP packets.
#[derive(Debug)]
pub enum TftpPacket {
    Rrq { filename: String, mode: String },
    Wrq { filename: String, mode: String },
    Data { block_num: u16, data: Vec<u8> },
    Ack { block_num: u16 },
    Error { error_code: u16, error_msg: String },
}

impl TftpPacket {
    /// Converts TFTPPacket instance to a raw byte stream.
    pub fn serialize(&self) -> Result<Vec<u8>, String> {
        let mut buf: Vec<u8>;

        match self {
            TftpPacket::Rrq { filename, mode } => {
                buf = serialize_rrq_wrq(OPCODE_RRQ, filename, mode)?;
            }
            TftpPacket::Wrq { filename, mode } => {
                buf = serialize_rrq_wrq(OPCODE_WRQ, filename, mode)?;
            }
            TftpPacket::Data { block_num, data } => {
                buf = Vec::with_capacity((2 * size_of::<u16>()) + data.len());

                buf.extend_from_slice(&OPCODE_DATA.to_be_bytes());
                buf.extend_from_slice(&block_num.to_be_bytes());
                buf.extend_from_slice(&data);
            }
            TftpPacket::Ack { block_num } => {
                buf = Vec::with_capacity(2 * size_of::<u16>());

                buf.extend_from_slice(&OPCODE_ACK.to_be_bytes());
                buf.extend_from_slice(&block_num.to_be_bytes());
            }
            TftpPacket::Error {
                error_code,
                error_msg,
            } => {
                if error_msg.len() > ERROR_MSG_MAXLEN {
                    return Err(format!(
                        "Error message too long. (Max allowed: {} bytes, Provided: {} bytes)",
                        ERROR_MSG_MAXLEN,
                        error_msg.len()
                    ));
                }

                buf = Vec::with_capacity((2 * size_of::<u16>()) + error_msg.len());

                buf.extend_from_slice(&OPCODE_ERROR.to_be_bytes());
                buf.extend_from_slice(&error_code.to_be_bytes());
                buf.extend_from_slice(error_msg.as_bytes());
                buf.push(0);
            }
        };

        Ok(buf)
    }

    /// Converts a raw byte stream into an instance of TFTPPacket.
    pub fn deserialize(byte_arr: &[u8]) -> Result<TftpPacket, String> {
        let mut buf: &[u8] = byte_arr;
        let packet: TftpPacket;

        let opcode: u16 = u16::from_be_bytes(
            buf.get(..size_of::<u16>())
                .ok_or_else(|| INVALID_DATA_ERROR.to_owned())?
                .try_into()
                .map_err(|e: TryFromSliceError| e.to_string())?,
        );

        buf = &buf[size_of::<u16>()..];

        match opcode {
            OPCODE_RRQ | OPCODE_WRQ => {
                let (filename, mode): (String, String) = deserialize_rrq_wrq(buf)?;

                packet = match opcode {
                    OPCODE_RRQ => TftpPacket::Rrq {
                        filename: filename,
                        mode: mode,
                    },
                    OPCODE_WRQ => TftpPacket::Wrq {
                        filename: filename,
                        mode: mode,
                    },
                    _ => unreachable!(),
                };
            }
            OPCODE_DATA => {
                let block_num: u16 = u16::from_be_bytes(
                    buf.get(..size_of::<u16>())
                        .ok_or_else(|| CORRUPTED_DATA_ERROR.to_owned())?
                        .try_into()
                        .map_err(|e: TryFromSliceError| e.to_string())?,
                );

                let data: Vec<u8> = buf[size_of::<u16>()..].to_vec();

                packet = TftpPacket::Data {
                    block_num: block_num,
                    data: data,
                };
            }
            OPCODE_ACK => {
                if buf.len() > size_of::<u16>() {
                    return Err(CORRUPTED_DATA_ERROR.to_owned());
                }

                let block_num: u16 = u16::from_be_bytes(
                    buf.get(..size_of::<u16>())
                        .ok_or_else(|| CORRUPTED_DATA_ERROR.to_owned())?
                        .try_into()
                        .map_err(|e: TryFromSliceError| e.to_string())?,
                );

                packet = TftpPacket::Ack {
                    block_num: block_num,
                };
            }
            OPCODE_ERROR => {
                let error_code: u16 = u16::from_be_bytes(
                    buf.get(..size_of::<u16>())
                        .ok_or_else(|| CORRUPTED_DATA_ERROR.to_owned())?
                        .try_into()
                        .map_err(|e: TryFromSliceError| e.to_string())?,
                );

                buf = &buf[size_of::<u16>()..];

                let end_index: usize = buf
                    .iter()
                    .position(|byte: &u8| *byte == b'\0')
                    .ok_or_else(|| CORRUPTED_DATA_ERROR.to_owned())?;

                let error_msg: String = str::from_utf8(&buf[..end_index])
                    .map_err(|e: Utf8Error| e.to_string())?
                    .to_owned();

                if end_index + 1 < buf.len() {
                    return Err(CORRUPTED_DATA_ERROR.to_owned());
                }

                packet = TftpPacket::Error {
                    error_code: error_code,
                    error_msg: error_msg,
                };
            }
            _ => {
                return Err(INVALID_DATA_ERROR.to_owned());
            }
        }

        Ok(packet)
    }
}

/// A custom error type for TFTP that holds both IO errors and custom String error messages.
/// ```io::Error```, ```String``` and ```&str``` values can be automatically converted to
/// ```TftpError```, using ```?``` or ```.into()``` method, via the implementation of
/// ```From<...>``` trait.
pub enum TftpError {
    Io(io::Error),
    /* The first argument is the message, the 2nd one is whether it is fatal or not. */
    Msg(String, bool),
}

impl TftpError {
    pub fn is_fatal(&self) -> bool {
        match self {
            TftpError::Msg(_, status) => *status,
            TftpError::Io(io_error) => match io_error.kind() {
                ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted => false,
                _ => true,
            },
        }
    }
}

impl From<io::Error> for TftpError {
    fn from(value: io::Error) -> Self {
        TftpError::Io(value)
    }
}

/*
 * When String and &str are converted into TftpError, they are assumed to be non fatal.
 * Fatal errors must be specified explicitly.
 */
impl From<String> for TftpError {
    fn from(value: String) -> Self {
        TftpError::Msg(value, false)
    }
}

impl From<&str> for TftpError {
    fn from(value: &str) -> Self {
        TftpError::Msg(value.to_owned(), false)
    }
}

impl fmt::Display for TftpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TftpError::Io(error) => write!(f, "IoError - {}: {}", error.kind(), error.to_string()),
            TftpError::Msg(msg, _) => write!(f, "Custom: {}", msg),
        }
    }
}

/****************************/
/* Public utility functions */
/****************************/
pub fn ascii_to_netascii(ascii_string: &str) -> String {
    if ascii_string.contains("\r\n") {
        ascii_string.to_owned()
    } else {
        ascii_string.replace("\n", "\r\n")
    }
}

pub fn netascii_to_ascii(netascii_string: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        netascii_string.to_owned()
    }

    #[cfg(not(target_os = "windows"))]
    {
        netascii_string.replace("\r\n", "\n")
    }
}

/// Checks if the filename is a valid filename. A valid filename is a string identified by only
/// name. No path is allowed and name shouldn't contain special paths/names like current directory
/// (.), parent directory (..), home directory (~) etc.
pub fn is_valid_filename(filename: &str) -> bool {
    !(filename.contains("/")
        || filename.contains("\\")
        || filename.contains("..")
        || filename == ".")
}

/***************************/
/* Local utility functions */
/***************************/

/// Utility function for serializing RRQ and WRQ packets.
fn serialize_rrq_wrq(opcode: u16, filename: &str, mode: &str) -> Result<Vec<u8>, String> {
    let mut err_msg: String = String::new();

    /* Checking whether file name respects the length restriction. */
    if filename.len() > FILENAME_MAXLEN {
        err_msg.push_str(&format!(
            "Internal error: File name too long. (Max allowed: {} bytes, Provided: {} bytes)",
            FILENAME_MAXLEN,
            filename.len()
        ));
    }

    /*
    * Checking whether the mode is a valid one.
    * Valid modes are (case-insensitive):
      1. netascii.
      2. octet.
      3. mail.
    */
    match mode.to_ascii_lowercase().as_ref() {
        "netascii" | "octet" | "mail" => {}
        _ => {
            /* Push a newline if there is already a message. */
            if err_msg.len() > 0 {
                err_msg.push('\n');
            }

            err_msg.push_str(&format!(
                "Internal error: Invalid mode. Mode must be \"netascii\", \
                \"octet\" or \"mail\" (case-insensitive)."
            ));
        }
    }

    /* Return error only when there is an error message. */
    if err_msg.len() > 0 {
        return Err(err_msg);
    }

    /* Populate the byte array buffer. */
    let mut buf: Vec<u8> = Vec::new();

    buf.reserve(size_of::<u16>() + filename.len() + mode.len() + 2);

    buf.extend_from_slice(&opcode.to_be_bytes());
    buf.extend_from_slice(filename.as_bytes());
    buf.push(0);
    buf.extend_from_slice(mode.as_bytes());
    buf.push(0);

    Ok(buf)
}

/// Utility function for deserializing raw byte stream of RRQ and WRQ packets.
fn deserialize_rrq_wrq(byte_arr: &[u8]) -> Result<(String, String), String> {
    let filename: String;
    let mode: String;

    let mut buf: &[u8] = byte_arr;
    let mut end_index: usize;

    /* Fetch file name. */
    end_index = buf
        .iter()
        .position(|byte: &u8| *byte == b'\0')
        .ok_or_else(|| CORRUPTED_DATA_ERROR.to_owned())?;

    filename = str::from_utf8(&buf[..end_index])
        .map_err(|e: Utf8Error| e.to_string())?
        .to_owned();

    buf = &buf[(end_index + 1)..];

    /* Fetch mode. */
    end_index = buf
        .iter()
        .position(|byte: &u8| *byte == b'\0')
        .ok_or_else(|| CORRUPTED_DATA_ERROR.to_owned())?;

    mode = str::from_utf8(&buf[..end_index])
        .map_err(|e: Utf8Error| e.to_string())?
        .to_ascii_lowercase();

    /* Check if mode is valid. */
    match mode.as_ref() {
        "netascii" | "octet" | "mail" => {}
        _ => {
            return Err("Invalid mode received.".to_owned());
        }
    }

    buf = &buf[(end_index + 1)..];

    /* TODO: Parsing options... */
    /* For now, return error for any extra data. This will change in the future. */
    if !buf.is_empty() {
        return Err(CORRUPTED_DATA_ERROR.to_owned());
    }

    Ok((filename, mode))
}

/*
* Opcode checking from raw data to avoid deserialization of whole packet before confirming
  validity, since deserialization can be costly.
*/
/// Returns a 16-bit unsigned integer from the first 2 bytes of ```buf```, assumed to be arranged
/// in network order. Returns ```Err(...)``` if ```buf``` is less than 2 bytes in length.
pub fn opcode_from_raw_data(buf: &[u8]) -> Result<u16, String> {
    let opcode: u16 = u16::from_be_bytes(
        buf.get(..size_of::<u16>())
            .ok_or_else(|| INVALID_DATA_ERROR.to_string())?
            .try_into()
            .map_err(|e: TryFromSliceError| e.to_string())?,
    );

    Ok(opcode)
}

/* The following read and write functions allows a set amount of retries for timeouts. */

/// Helper function for sending/writing over UDP, allowing a finite number of retries in case
/// of timeouts. If ```target_addr_opt``` is ```Some(target_addr)```, the function internally
/// calls ```send_to(...)``` method for unconnected ```send/write``` to ```target_addr```
/// and calls ```send(...)```
/// method for connected ```send/write```, provided that the ```socket``` is successfully
/// connected to a valid peer address.
pub fn send_retry(
    socket: &UdpSocket,
    target_addr_opt: Option<SocketAddr>,
    buffer: &[u8],
    retry_count: u64,
) -> Result<usize, TftpError> {
    for _ in 0..retry_count {
        let wr_size_opt: Result<usize, Error> = if let Some(target_addr) = target_addr_opt {
            socket.send_to(buffer, target_addr)
        } else {
            socket.send(buffer)
        };

        match wr_size_opt {
            Ok(written_size) => {
                return Ok(written_size);
            }
            Err(err)
                if err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(err) => {
                return Err(err.into());
            }
        }
    }

    /* If the function comes here, the connection has been timed out. */
    return Err("Connection timed out.".into());
}

/// Helper function for receiving/reading over UDP, allowing a finite number of retries in case
/// of timeouts. Based on whether ```connected``` is ```true``` or ```false```, the function
/// internally calls ```recv_from(...)``` method (for unconnected ```recv```) or ```recv(...)```
/// (for connected ```recv```) method. If ```connected``` is ```true```, the function receives
/// from the peer connected to the socket and returns ```(received_size, None)```. Otherwise,
/// it receives a packet from any peer and returns ```(received_size, Some(peer_addr))```.
pub fn recv_retry(
    socket: &UdpSocket,
    buffer: &mut [u8],
    retry_count: u64,
    connected: bool,
) -> Result<(usize, Option<SocketAddr>), TftpError> {
    for _ in 0..retry_count {
        let rd_size_n_peer_addr_opt: Result<(usize, Option<SocketAddr>), Error> = if connected {
            socket.recv(buffer).map(|rd_size: usize| (rd_size, None))
        } else {
            socket
                .recv_from(buffer)
                .map(|(rd_size, peer_addr): (usize, SocketAddr)| (rd_size, Some(peer_addr)))
        };

        match rd_size_n_peer_addr_opt {
            Ok(rd_size_n_peer_addr) => {
                return Ok(rd_size_n_peer_addr);
            }
            Err(err)
                if err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(err) => {
                return Err(err.into());
            }
        }
    }

    /* If the function comes here, the connection has been timed out. */
    return Err("Connection timed out.".into());
}

/// Return appropriate error message from error code found in ERROR packet.
pub fn error_msg_from_code(error_code: u16) -> String {
    match error_code {
        ERROR_CODE_ACCESS_VIOLATION => "Permission denied.".to_owned(),
        ERROR_CODE_DISK_FULL_OR_ALLOC_EXCEEDED => "Out of space on disk.".to_owned(),
        ERROR_CODE_FILE_EXISTS => "File already exists.".to_owned(),
        ERROR_CODE_FILE_NOT_FOUND => "Requested file not found.".to_owned(),
        ERROR_CODE_ILLEGAL_OP => "Illegal operation.".to_owned(),
        ERROR_CODE_UNKNOWN_TID => "Response from unknown TID.".to_owned(),
        ERROR_CODE_NO_SUCH_USER => "No such user found.".to_owned(),
        _ => "Unknown error.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use crate::core::TftpPacket;

    #[test]
    fn serialize_deserialize() {
        eprintln!("TEST\n");

        let packet: TftpPacket = TftpPacket::Wrq {
            filename: "random.txt".to_owned(),
            mode: "netascii".to_owned(),
        };

        eprintln!("Before serialization:\n{:?}\n", packet);

        let buf: Vec<u8> = packet.serialize().unwrap();
        eprintln!("Serialized array:\n{:?}\n", buf);

        let pkt: TftpPacket = TftpPacket::deserialize(&buf).unwrap();
        eprintln!("Deserialized packet:\n{:?}\n", pkt);
    }
}
