use std::{
    collections::VecDeque,
    net::{AddrParseError, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use crate::core::{
    DEFAULT_RETRY_COUNT, DEFAULT_TIMEOUT_SEC, ERROR_CODE_ILLEGAL_OP, ERROR_CODE_SEE_MSG,
    ETH_FRAME_LEN, FILENAME_ERROR, INVALID_DATA_ERROR, OPCODE_ACK, OPCODE_ERROR, TftpError,
    TftpPacket, error_msg_from_code, is_valid_filename, opcode_from_raw_data, recv_retry,
    send_retry,
    transmission::{receive_file, send_file},
};

macro_rules! set_option {
    ($arg:expr, $option:expr, $obj:expr, $err_list:expr) => {
        if $obj.is_some() {
            $err_list.push(format!("{} redefined.", $arg));
            continue;
        }

        $option = Some($arg);
    };
}

const FILENAME_MAXLEN: usize = 128;
const TFTP_PORT_DEFAULT: u16 = 69;

#[derive(Debug)]
pub enum TftpAction {
    Get,
    Put,
}

/***********************************/
/* Core functions for TFTP client. */
/***********************************/

/// Parses command line arguments for TFTP client CLI utility.
pub fn parse_args(
    args: &[String],
) -> Result<Option<(TftpAction, String, String, SocketAddr)>, String> {
    /* "args" is the argument list with the first entry, which is the program name, removed. */
    if args.len() == 0 {
        return Ok(None);
    }

    let mut expect_option = true;
    let mut error_list: Vec<String> = Vec::new();

    let mut tftp_action: Option<TftpAction> = None;
    let mut src_filename: Option<String> = None;
    let mut dest_filename: Option<String> = None;
    let mut server_addr: Option<SocketAddr> = None;

    let mut option: Option<&str> = None;
    let mut arg_q: VecDeque<&String> = VecDeque::from_iter(args.iter());

    while let Some(arg) = arg_q.pop_front() {
        if expect_option {
            /* Check for valid option. */

            /* Options always start with a hyphen. */
            if arg.chars().nth(0).unwrap() != '-' {
                error_list.push(format!("Unknown value: {}", arg));
                continue;
            }

            /* Checking options. */
            match arg.as_str() {
                "--get" => {
                    if tftp_action.is_some() {
                        error_list.push(format!("Request type redefined."));
                        continue;
                    }

                    tftp_action = Some(TftpAction::Get);
                }
                "--put" => {
                    if tftp_action.is_some() {
                        error_list.push(format!("Request type redefined."));
                        continue;
                    }

                    tftp_action = Some(TftpAction::Put);
                }
                "-s" => {
                    set_option!(arg.as_str(), option, src_filename, error_list);
                    expect_option = false;
                }
                "-d" => {
                    set_option!(arg.as_str(), option, dest_filename, error_list);
                    expect_option = false;
                }
                "-t" => {
                    set_option!(arg.as_str(), option, server_addr, error_list);
                    expect_option = false;
                }
                _ => {
                    error_list.push(format!("Unknown option: {}", arg));
                    continue;
                }
            }
        } else {
            /* Check for valid value for options. */
            if arg.chars().nth(0).unwrap() == '-' {
                /*
                * If an option is provided instead of a value, push error string,
                  set expect_option to true, push the argument to the front of the
                  argument list for further inspection and jump to the beginning of the loop.
                */
                error_list.push(format!("Value not found for option {}.", option.unwrap()));
                option = None;
                expect_option = true;
                arg_q.push_front(arg);
                continue;
            }

            /* Assign values to appropriate variable based on "option". */
            match option {
                None => {
                    error_list.push(format!("Unknown value: {}", arg));
                    expect_option = true;
                    continue;
                }
                /* Source file name. */
                Some("-s") | Some("-d") => {
                    if arg.len() > FILENAME_MAXLEN {
                        return Err(format!(
                            "{} file name exceeds maximum allowed length (Allowed: 128 bytes\
                            , Got: {} bytes).",
                            if option.unwrap() == "-s" {
                                "Source"
                            } else {
                                "Destination"
                            },
                            arg.len()
                        ));
                    }

                    if option.unwrap() == "-s" {
                        src_filename = Some(arg.to_owned());
                    } else {
                        dest_filename = Some(arg.to_owned());
                    }

                    expect_option = true;
                }
                Some("-t") => {
                    /*
                    * If successfully converted to Ipv4Addr, create SocketAddr from it and return.
                    * In case of failure, push error, no need to return anything,
                      since we are going to discard it anyway (converting to Option<SocketAddr>).
                    */
                    server_addr = arg
                        .parse::<Ipv4Addr>()
                        .map(|ipv4_addr: Ipv4Addr| {
                            SocketAddr::V4(SocketAddrV4::new(ipv4_addr, TFTP_PORT_DEFAULT))
                        })
                        .map_err(|e: AddrParseError| error_list.push(e.to_string()))
                        .ok();

                    expect_option = true;
                }
                _ => {
                    error_list.push(format!("Invalid option: {}", option.unwrap()));
                    error_list.push(format!("Unknown value: {}", arg));
                    expect_option = true;
                    continue;
                }
            }
        }
    }

    if tftp_action.is_none() {
        error_list.push(format!(
            "Request type is not specified (must be --get or --put)."
        ));
    }

    if src_filename.is_none() {
        error_list.push(format!(
            "Source file name is not specified (must be passed with -s option.)"
        ));
    }

    if dest_filename.is_none() {
        error_list.push(format!(
            "Destination file name is not specified (must be passed with -d option.)"
        ));
    }

    if server_addr.is_none() {
        error_list.push(format!(
            "Valid target address not provided (must be passed with -t option)."
        ));
    }

    if !error_list.is_empty() {
        return Err(error_list.join("\n"));
    }

    Ok(Some((
        tftp_action.unwrap(),
        src_filename.unwrap(),
        dest_filename.unwrap(),
        server_addr.unwrap(),
    )))
}

/// Handles read requests (RRQ) from client to server.
pub fn get_file(
    src_filename: String,
    dest_filename: String,
    server_addr: SocketAddr,
) -> Result<(), TftpError> {
    if !is_valid_filename(&src_filename) || !is_valid_filename(&dest_filename) {
        return Err(FILENAME_ERROR.into());
    }

    let socket: UdpSocket = UdpSocket::bind("0.0.0.0:0")?;

    /* Set timeout. */
    socket.set_read_timeout(Some(Duration::from_secs(DEFAULT_TIMEOUT_SEC)))?;
    socket.set_write_timeout(Some(Duration::from_secs(DEFAULT_TIMEOUT_SEC)))?;

    /* For now, everything will be transmitted in octet mode. Others may be implemented later. */
    let mode: &str = "octet";
    let packet: TftpPacket;

    /* Send an RRQ packet to the server. Send to server at port 69. */
    packet = TftpPacket::Rrq {
        filename: src_filename,
        mode: mode.to_owned(),
    };

    send_retry(
        &socket,
        Some(server_addr),
        &packet.serialize()?,
        DEFAULT_RETRY_COUNT,
    )?;

    let mut total_received_size: usize = 0;

    receive_file(
        Arc::new(PathBuf::from(".")),
        &dest_filename,
        mode,
        &socket,
        Some(server_addr),
        |block_num: u16, block_size: usize| {
            total_received_size += block_size;

            if block_num > 1 {
                /* Move cursor 2 lines up, to refresh the contents. */
                print!("\x1b[2A");
            }

            println!(
                "Received block: {}\nReceived size: {} byte(s)",
                block_num, total_received_size
            );
        },
    )
}

/// Handles write requests (WRQ) to server from client.
pub fn put_file(
    src_filename: String,
    dest_filename: String,
    server_addr: SocketAddr,
) -> Result<(), TftpError> {
    if !is_valid_filename(&src_filename) || !is_valid_filename(&dest_filename) {
        return Err(FILENAME_ERROR.into());
    }

    let socket: UdpSocket = UdpSocket::bind("0.0.0.0:0")?;

    /* Set timeout. */
    socket.set_read_timeout(Some(Duration::from_secs(DEFAULT_TIMEOUT_SEC)))?;
    socket.set_write_timeout(Some(Duration::from_secs(DEFAULT_TIMEOUT_SEC)))?;

    /* For now, everything will be transmitted in octet mode. Others may be implemented later. */
    let mode: &str = "octet";
    let packet: TftpPacket;

    /* Send an WRQ packet to the server. Send to server at port 69. */
    packet = TftpPacket::Wrq {
        filename: dest_filename,
        mode: mode.to_owned(),
    };

    send_retry(
        &socket,
        Some(server_addr),
        &packet.serialize()?,
        DEFAULT_RETRY_COUNT,
    )?;

    let mut buf: [u8; ETH_FRAME_LEN] = [0; ETH_FRAME_LEN];
    let mut rd_size: usize;

    /* Discard packets until we get a packet from server_addr. */
    loop {
        let sender_addr: SocketAddr;
        (rd_size, sender_addr) = recv_retry(&socket, &mut buf, DEFAULT_RETRY_COUNT, false).map(
            |(rd_size, sender_addr_opt): (usize, Option<SocketAddr>)| {
                (rd_size, sender_addr_opt.unwrap())
            },
        )?;

        /*
        * If we get a packet from desired IP address, connect socket to that IP and break the
          loop.
        */
        if sender_addr.ip() == server_addr.ip() {
            socket.connect(sender_addr)?;
            break;
        }

        /* If the IP doesn't match the desired server IP, send an ERROR packet for unknown TID. */
        let error_pkt: TftpPacket = TftpPacket::Error {
            error_code: ERROR_CODE_ILLEGAL_OP,
            error_msg: "Unknown TID.".to_owned(),
        };

        send_retry(&socket, Some(sender_addr), &error_pkt.serialize()?, 1)?;
    }

    /* Check if it is an ACK packet with data block set to 0. */
    let recv_packet: TftpPacket = match opcode_from_raw_data(&buf[..size_of::<u16>()])? {
        OPCODE_ACK | OPCODE_ERROR => TftpPacket::deserialize(&buf[..rd_size])?,
        _ => {
            return Err(INVALID_DATA_ERROR.into());
        }
    };

    match recv_packet {
        TftpPacket::Ack { block_num } => {
            if block_num == 0 {
                let mut sent_size: usize = 0;

                send_file(
                    Arc::new(PathBuf::from(".")),
                    &src_filename,
                    mode,
                    &socket,
                    |block_num: u16, block_size: usize| {
                        sent_size += block_size;

                        if block_num > 1 {
                            print!("\x1b[2A");
                        }

                        println!(
                            "Sent block: {}\nSent size: {} byte(s).",
                            block_num, sent_size
                        );
                    },
                )?;
            } else {
                let (err_code, err_msg): (u16, String) = (
                    ERROR_CODE_ILLEGAL_OP,
                    error_msg_from_code(ERROR_CODE_ILLEGAL_OP),
                );

                let packet: TftpPacket = TftpPacket::Error {
                    error_code: err_code,
                    error_msg: err_msg.clone(),
                };

                send_retry(&socket, None, &packet.serialize()?, 1)?;
                return Err(err_msg.into());
            }
        }
        TftpPacket::Error {
            error_code,
            error_msg,
        } => {
            if error_code == ERROR_CODE_SEE_MSG {
                return Err(error_msg.into());
            }

            return Err(error_msg_from_code(error_code).into());
        }
        _ => {
            /*
            * In case of any other type of packets, return ERROR packet of type 5
              (Illegal operation).
            * The packet doesn't need to be retransmitted in case of failure.
            */
            let error_pkt: TftpPacket = TftpPacket::Error {
                error_code: ERROR_CODE_ILLEGAL_OP,
                error_msg: "Illegal operation.".to_owned(),
            };

            send_retry(&socket, None, &error_pkt.serialize()?, 1)?;

            return Err("Invalid packet received.".into());
        }
    }

    Ok(())
}

pub fn help_info_tftpcl() {
    println!(
        "\
        \"tftpcl\" TFTP client command line utility.\n\
        Syntax:  tftpcl [Option]... [Value]...\n\
        Mandatory options:\n\
        \t-s src_filename           Sets the name of the source file that has to be transferred.\n\
        \t-d dest_filename          Sets the name the file received by client or server is to be\n\
        \t                          saved in.\n\
        \t-t target_addr            Sets the address of the target (server) machine.\n\
        \t--get                     Receives/downloads a file with src_filename in the target/server\n\
        \t                          machine. The file is saved with the name dest_filename after\n\
        \t                          receiving.\n\
        \t--put                     Sends/uploads a file with src_filename from the client machine\n\
        \t                          to the target/server machine. The file is saved there with the\n\
        \t                          name dest_name.\n\
        Optional options:\n\
        \t--help                    Show help page.\
        "
    );
}
