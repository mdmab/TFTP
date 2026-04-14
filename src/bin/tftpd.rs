use std::{
    env::args,
    io::Error,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::PathBuf,
    sync::Arc,
    thread,
};

use tftp::{
    core::{
        ERROR_CODE_ILLEGAL_OP, ERROR_CODE_SEE_MSG, ETH_FRAME_LEN, INVALID_DATA_ERROR, OPCODE_ACK,
        OPCODE_DATA, OPCODE_ERROR, OPCODE_RRQ, OPCODE_WRQ, TftpError, TftpPacket,
        error_msg_from_code, opcode_from_raw_data, recv_retry, send_retry,
    },
    elog, elog_fatal,
    tftpd_util::{help_info_tftpd, parse_args, receive_file, send_file},
};

fn main() {
    /* Parsing args. */
    let arg_list: Vec<String> = args().collect();
    let root_path: Arc<PathBuf> = match parse_args(&arg_list[1..]) {
        Err(err) => {
            elog_fatal!(-1, "{}", err);
        }
        Ok(None) => {
            help_info_tftpd();
            return;
        }
        Ok(Some(path)) => Arc::new(path),
    };

    /* Default listener socket at port 69. */
    const PORT: u16 = 69;
    let server_addr: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT));
    let sock_tftpd: UdpSocket =
        UdpSocket::bind(server_addr).expect("Failed to create UDP port for TFTPD");

    println!("Listening to {}...", sock_tftpd.local_addr().unwrap());

    let mut buf: [u8; ETH_FRAME_LEN] = [0; ETH_FRAME_LEN];

    /*
    * Inside the loop, listen to the default port 69 for any incoming RRQ/WRQ requests. If a
      valid request arrives, send a positive response from another port that will be used for
      connection with the client. Port 69 should only listen for requests, nothing else.
    * In case of errors, do not terminate the connection unless it is a fatal error (errors
      raised by OS/socket on the default socket, rather than client packets or server sockets
      dedicated to a client).
    */
    loop {
        /* Receive the packet. */
        let (rd_size, client_addr): (usize, SocketAddr) =
            match recv_retry(&sock_tftpd, &mut buf, 1, false) {
                Err(err) if err.is_fatal() => {
                    /* Consider it to be fatal. Log the error and terminate. */
                    elog_fatal!(-1, "{}", err.to_string());
                }
                Err(err) => {
                    /* Log the error into server and continue for another packet. */
                    elog!("{}", err);
                    continue;
                }
                Ok((rd_size, Some(addr))) => (rd_size, addr),
                _ => {
                    /* It is an unconnected connection, so this arm is unreachable. */
                    unreachable!();
                }
            };

        /*
        * Check if the opcode indicates it to be an RRQ/WRQ/ERROR packet. If any other packet
          arrvies, send an error packet and continue looking for next valid request.
        */
        match opcode_from_raw_data(&buf[..rd_size]) {
            Err(err) => {
                elog!("{}", err);
                continue;
            }
            Ok(OPCODE_RRQ) | Ok(OPCODE_WRQ) | Ok(OPCODE_ERROR) => {}
            Ok(opc) => {
                let err_code: u16;
                let err_msg: String;

                (err_code, err_msg) = match opc {
                    OPCODE_ACK | OPCODE_DATA => (
                        ERROR_CODE_ILLEGAL_OP,
                        error_msg_from_code(ERROR_CODE_ILLEGAL_OP),
                    ),
                    _ => (ERROR_CODE_SEE_MSG, INVALID_DATA_ERROR.to_string()),
                };

                let err_pkt: TftpPacket = TftpPacket::Error {
                    error_code: err_code,
                    error_msg: err_msg.clone(),
                };

                match send_retry(
                    &sock_tftpd,
                    Some(client_addr),
                    &err_pkt.serialize().unwrap(),
                    1,
                ) {
                    Ok(_) => {}
                    Err(err) if err.is_fatal() => {
                        elog_fatal!(-1, "{}", err);
                    }
                    Err(err) => {
                        elog!("{}", err);
                        continue;
                    }
                }

                elog!("{}", err_msg);
                continue;
            }
        }

        /*
        * If RRQ/WRQ packet, create an UDP socket using a random, OS-picked port, connect to the
          client and use this connection for the entirety of the transfer.
        * If ERROR packet, show the error message and terminate the connection and look for another
          connection.
        */
        let packet: TftpPacket = match TftpPacket::deserialize(&buf[..rd_size]) {
            Err(err) => {
                elog!("{}", err);
                continue;
            }
            Ok(packet) => packet,
        };

        /*
         * Creating new UDP connection at random open port in the server for connection.
         * This socket will be dedicated to one client only.
         */
        let socket: UdpSocket =
            match UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))
                .map_err(|err: Error| TftpError::from(err))
            {
                Ok(socket) => socket,
                Err(err) if err.is_fatal() => {
                    elog_fatal!(-1, "{}", err);
                }
                Err(err) => {
                    elog!("{}", err);
                    continue;
                }
            };

        match socket.connect(client_addr) {
            Err(err) => {
                elog!("{}", err);
                continue;
            }
            _ => {}
        }

        /* Calling appropriate routine for appropritate request. */
        // TODO: Ability to set root folder.
        let root_dir: Arc<PathBuf> = root_path.clone();

        match packet {
            TftpPacket::Error {
                error_code,
                error_msg,
            } => {
                elog!("Error from client - Code {}: {}", error_code, error_msg);
                continue;
            }
            TftpPacket::Rrq { filename, mode } => {
                thread::spawn(move || {
                    if let Err(err) = send_file(root_dir, &filename, &mode, &socket) {
                        elog!("{}", err);
                        elog!("Failed to send {} to {}", filename, client_addr);
                    } else {
                        elog!("Successfully sent {} to {}.", filename, client_addr);
                    }
                });
            }
            TftpPacket::Wrq { filename, mode } => {
                thread::spawn(move || {
                    if let Err(err) = receive_file(root_dir, &filename, &mode, &socket) {
                        elog!("{}", err);
                        elog!("Failed to receive {} from {}", filename, client_addr);
                    } else {
                        elog!("Successfully received {} from {}.", filename, client_addr);
                    }
                });
            }
            _ => {
                unreachable!();
            }
        }
    }
}
