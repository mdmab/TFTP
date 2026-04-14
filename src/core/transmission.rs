//! This module contains the core routines for transmission of file (both sending and receiving).

use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Error, ErrorKind, Read, Write},
    net::{SocketAddr, UdpSocket},
    path::PathBuf,
    sync::Arc,
};

use crate::core::{
    DATA_BLOCK_SIZE, DEFAULT_RETRY_COUNT, ERROR_CODE_ACCESS_VIOLATION, ERROR_CODE_FILE_NOT_FOUND,
    ERROR_CODE_ILLEGAL_OP, ERROR_CODE_SEE_MSG, ETH_FRAME_LEN, INVALID_DATA_ERROR, OPCODE_ACK,
    OPCODE_DATA, OPCODE_ERROR, OPCODE_RRQ, OPCODE_WRQ, TftpError, TftpPacket, error_msg_from_code,
    opcode_from_raw_data, recv_retry, send_retry,
};

/*
* This function handles the transmission of file from server to remote peer, through sending
  DATA packets and receiving ACK packets.
*/
pub fn send_file<F>(
    root_dir: Arc<PathBuf>,
    filename: &str,
    mode: &str,
    socket: &UdpSocket,
    mut on_send: F,
) -> Result<(), TftpError>
where
    F: FnMut(u16, usize) -> (),
{
    /* Check for valid mode. */
    match mode {
        "octet" => {}
        "netascii" | "mail" => {
            return Err(format!("Mode \"{}\" not supported yet.", mode).into());
        }
        _ => return Err(format!("Invalid mode \"{}\".", mode).into()),
    }

    /* Right now, implement only for octet mode. */

    /*
    * The RRQ request from client has already been processed. Open the file, identified by the
      filename.
    * If the file couldn't be opened due to some reason, send an error packet without any
      retransmission and terminate the connection.
    */
    let file_path: PathBuf = root_dir.as_ref().join(filename);

    let file: File = match OpenOptions::new()
        .read(true)
        .truncate(false)
        .write(false)
        .create(false)
        .append(false)
        .open(file_path)
    {
        Ok(file) => file,
        Err(err) => {
            let error_code: u16;
            let error_msg: String;

            (error_code, error_msg) = match err.kind() {
                ErrorKind::NotFound | ErrorKind::IsADirectory => (
                    ERROR_CODE_FILE_NOT_FOUND,
                    error_msg_from_code(ERROR_CODE_FILE_NOT_FOUND),
                ),
                ErrorKind::PermissionDenied => (
                    ERROR_CODE_ACCESS_VIOLATION,
                    error_msg_from_code(ERROR_CODE_ACCESS_VIOLATION),
                ),
                _ => (
                    ERROR_CODE_SEE_MSG,
                    "Server failed to find/open file.".to_owned(),
                ),
            };

            let error_pkt: TftpPacket = TftpPacket::Error {
                error_code: error_code,
                error_msg: error_msg.clone(),
            };

            send_retry(socket, None, &error_pkt.serialize()?, 1)?;
            return Err(err.into());
        }
    };

    let mut file_buf: BufReader<File> = BufReader::new(file);

    /*
    * As a positive response to RRQ request, send blocks of file to the client, starting with
      block 1, of default size (512 bytes).
    */
    let mut byte_arr: [u8; ETH_FRAME_LEN] = [0u8; ETH_FRAME_LEN];
    let mut expected_block_num: u16 = 1;
    let data_block_size: usize = DATA_BLOCK_SIZE;

    /* Send the block first and then wait for ACK packet for that block in each execution. */
    loop {
        /* Read file content and handle errors. */
        let data_rd_size: usize = match file_buf.read(&mut byte_arr[..data_block_size]) {
            Ok(n) => n,
            Err(err) => {
                /* Send an ERROR packet and terminate. */
                let error_msg: String = match err.kind() {
                    ErrorKind::PermissionDenied => {
                        "File permission changed in between operation.".to_owned()
                    }
                    ErrorKind::Interrupted => "Read operation interrupted.".to_owned(),
                    ErrorKind::UnexpectedEof => "File truncated in between operation.".to_owned(),
                    _ => "Error occurred.".to_owned(),
                };

                let error_pkt: TftpPacket = TftpPacket::Error {
                    error_code: ERROR_CODE_SEE_MSG,
                    error_msg: error_msg.clone(),
                };

                send_retry(socket, None, &error_pkt.serialize()?, 1)?;

                return Err(err.into());
            }
        };

        /* After reading, send the data block. */
        let data_pkt: TftpPacket = TftpPacket::Data {
            block_num: expected_block_num,
            data: byte_arr[..data_rd_size].to_owned(),
        };

        send_retry(socket, None, &data_pkt.serialize()?, DEFAULT_RETRY_COUNT)?;
        on_send(expected_block_num, data_rd_size);

        /*
        * Receive an ACK packet with the block number, handling errors on the way.
        * Do it inside a loop, because in case of duplicate ACK packet, we need to keep receiving
          unless we get the desired ACK packet, after which we break the loop.
        */
        'recv_loop: loop {
            let (ack_rd_size, _): (usize, _) =
                match recv_retry(socket, &mut byte_arr, DEFAULT_RETRY_COUNT, true) {
                    Err(err) => {
                        let error_pkt: TftpPacket = TftpPacket::Error {
                            error_code: ERROR_CODE_SEE_MSG,
                            error_msg: format!(
                                "Failed to receive acknowledgement for block {}.",
                                expected_block_num
                            ),
                        };

                        send_retry(socket, None, &error_pkt.serialize()?, 1)?;
                        return Err(err.into());
                    }
                    Ok(res) => res,
                };

            /*
            * Check if the received packet is ACK or ERROR packet.
            * Checking opcode before deserailzing since deserialization can be expensive for some
              packets.
            */
            match opcode_from_raw_data(&byte_arr[..ack_rd_size])? {
                OPCODE_ACK | OPCODE_ERROR => {}
                opc => {
                    let error_code: u16;
                    let error_msg: String;

                    (error_code, error_msg) = match opc {
                        OPCODE_DATA | OPCODE_RRQ | OPCODE_WRQ => (
                            ERROR_CODE_ILLEGAL_OP,
                            error_msg_from_code(ERROR_CODE_ILLEGAL_OP),
                        ),
                        _ => (ERROR_CODE_SEE_MSG, INVALID_DATA_ERROR.to_string()),
                    };

                    let error_pkt: TftpPacket = TftpPacket::Error {
                        error_code: error_code,
                        error_msg: error_msg.clone(),
                    };

                    send_retry(socket, None, &error_pkt.serialize()?, 1)?;
                    return Err(error_msg.into());
                }
            }

            /*
            * Deserialize and handle ACK or ERROR packet.
            * For ACK packets, check if the expected data packet has been acknowledgement. ACK for
              other data block numbers is an error.
            * For ERROR packets, show the error message and terminate the connection.
            */
            match TftpPacket::deserialize(&byte_arr[..ack_rd_size])? {
                TftpPacket::Ack { block_num } => {
                    if block_num == expected_block_num {
                        /*
                        * Expected block arrived. Next step depends on the size of the sent data
                           block.
                        * If the sent data block is of size less than data_block_size, this was the
                          last data block. Terminate the loop + connection.
                        * Otherwise, proceed to send the next data block.
                        */
                        if data_rd_size < data_block_size {
                            return Ok(());
                        }

                        expected_block_num += 1;
                        break 'recv_loop;
                    } else if block_num < expected_block_num {
                        /*
                        * Probably an ACK for previous block got lost. Discard and re-receive a
                          packet.
                        */
                        continue 'recv_loop;
                    } else {
                        /*
                        * Future DATA packet is a protocol violation. Send an ERROR packet
                          and terminate.
                        */
                        let (err_code, err_msg): (u16, String) = (
                            ERROR_CODE_ILLEGAL_OP,
                            error_msg_from_code(ERROR_CODE_ILLEGAL_OP),
                        );

                        let err_pkt: TftpPacket = TftpPacket::Error {
                            error_code: err_code,
                            error_msg: err_msg.clone(),
                        };

                        send_retry(socket, None, &err_pkt.serialize()?, 1)?;
                        return Err(err_msg.into());
                    }
                }
                TftpPacket::Error {
                    error_code,
                    error_msg,
                } => {
                    return Err(format!(
                        "Error from client - Error code {}: {}",
                        error_code, error_msg
                    )
                    .into());
                }
                _ => {
                    unreachable!();
                }
            }
        }
    }
}

/*
* This function handles the transmission of file from remote peer to server, through receiving
  DATA packets and sending ACK packets.
*/
pub fn receive_file<F>(
    root_dir: Arc<PathBuf>,
    dest_filename: &str,
    mode: &str,
    socket: &UdpSocket,
    sender_addr_opt: Option<SocketAddr>,
    mut on_recv: F,
) -> Result<(), TftpError>
where
    F: FnMut(u16, usize) -> (),
{
    let mut byte_arr: [u8; ETH_FRAME_LEN] = [0u8; ETH_FRAME_LEN];
    /*
    * Receive DATA packet from the server. Save the port, which will be used for all further
      transactions.
    * The first DATA packet will have block number set to 1.
    */

    let mut data_block_expected: u16 = 1;
    let mut rd_size: usize;

    /*
    * If there is a sender address provided, the socket in assumed unconnected and we have
      to manually search for the packet the comes from the sender's address.
    * If it is not provided, the socket is assumed connected and we will just receive packet.
    */
    if let Some(sender_addr) = sender_addr_opt {
        /* Receive packets and keep discarding them unless we get a packet from the server IP. */
        loop {
            /* Retrieve the peer address from all the retrieved packets. */
            let peer_addr: SocketAddr;
            (rd_size, peer_addr) = recv_retry(socket, &mut byte_arr, DEFAULT_RETRY_COUNT, false)
                .map(|(rd_size, peer_addr_opt): (usize, Option<SocketAddr>)| {
                    (rd_size, peer_addr_opt.unwrap())
                })?;

            /*
            * If the peer address matches the server address we provided, we have found the address
              and port we are supposed to be connected to for this session. Connect the socket to
              that remote address and break the loop.
            * If the peer address doesn't match the desired server address, we discard it, send an
              ERROR packet of type 5 (Unknow TID) and go for next packet.
            */
            if peer_addr.ip() == sender_addr.ip() {
                socket.connect(peer_addr)?;
                break;
            }

            let error_pkt: TftpPacket = TftpPacket::Error {
                error_code: ERROR_CODE_ILLEGAL_OP,
                error_msg: "Unknown TID.".to_owned(),
            };

            send_retry(&socket, Some(peer_addr), &error_pkt.serialize()?, 1)?;
        }
    } else {
        (rd_size, _) = recv_retry(socket, &mut byte_arr, DEFAULT_RETRY_COUNT, true)?;
    }

    /* Open a temporary file. On success, rename it to destination name. On error, remove it. */
    const TEMP_FILENAME: &str = ".tmp.tftpcl";
    let temp_file_path: PathBuf = root_dir.as_ref().join(TEMP_FILENAME);

    let file: File = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temp_file_path)
    {
        Ok(file) => file,
        Err(err) => {
            let (err_code, err_msg): (u16, String) = match err.kind() {
                ErrorKind::NotFound | ErrorKind::IsADirectory => (
                    ERROR_CODE_FILE_NOT_FOUND,
                    error_msg_from_code(ERROR_CODE_FILE_NOT_FOUND),
                ),
                ErrorKind::PermissionDenied => (
                    ERROR_CODE_ACCESS_VIOLATION,
                    error_msg_from_code(ERROR_CODE_ACCESS_VIOLATION),
                ),
                _ => (
                    ERROR_CODE_SEE_MSG,
                    "Server error when attempting to open file.".to_owned(),
                ),
            };

            let err_pkt: TftpPacket = TftpPacket::Error {
                error_code: err_code,
                error_msg: err_msg.clone(),
            };

            send_retry(socket, None, &err_pkt.serialize()?, 1)?;
            return Err(err_msg.into());
        }
    };

    let mut file_buf: BufWriter<File> = BufWriter::new(file);

    /* Now, receive DATA from the server or receive ERROR packet. */
    loop {
        /*
        * Inspect the first two bytes of the received packet to check if it contains the opcode
          for DATA or ERROR packets. If so, proceed, else terminate the connection.
        * Initially, the buffer (byte_arr) holds the content we fetch in the previous recv_from
          call, when we were searching for peer address to make a connection.
        */
        let recv_packet: TftpPacket = match opcode_from_raw_data(&byte_arr[..rd_size])? {
            OPCODE_DATA | OPCODE_ERROR => {
                TftpPacket::deserialize(&byte_arr[..rd_size]).map_err(|err: String| {
                    _ = fs::remove_file(&temp_file_path);
                    err
                })?
            }
            _ => {
                _ = fs::remove_file(&temp_file_path);
                return Err(INVALID_DATA_ERROR.into());
            }
        };

        /*
        * Check packets and decide whether to keep receiving or terminate the process.
        * If the packet is a DATA packet -
          - If the expected data block has arrived, send an ACK packet and repopulate byte_arr
            with content of the next packet from the server.
          - If the arrived data block number is lower than expected, the server probably missed
            ACK packet for a previous data block. Resend the ACK packet for that data block.
          - If the arrived data block number is greater than the expected one, the client probably
            lost some data from server. Do not send ACK packet. Just retrieve another packet from
            the server.
        * If the packet is an ERROR packet, show error message and terminate.
        * If the packet is any other type of packet, send an ERROR packet of type 5 (Illegal
          operation) and terminate. No need to retransmit the ERROR packet in case of failure.
        */
        match recv_packet {
            TftpPacket::Data { block_num, data } => {
                if block_num < data_block_expected {
                    /*
                    * If block number is less the expected one, the server is retransmitting
                      it, probably because previous ACK packet got lost. Resend ACK packet.
                    */
                    let ack_packet: TftpPacket = TftpPacket::Ack {
                        block_num: block_num,
                    };

                    send_retry(&socket, None, &ack_packet.serialize()?, DEFAULT_RETRY_COUNT)?;
                } else if block_num == data_block_expected {
                    on_recv(block_num, data.len());
                    /* Expected data block arrived. */

                    /* Append the received data to file. */
                    file_buf.write_all(&data).map_err(|err: Error| {
                        _ = fs::remove_file(&temp_file_path);
                        err.to_string()
                    })?;

                    /* Send an ACK packet to the server and add 1 to data_block_expected. */
                    let ack_packet: TftpPacket = TftpPacket::Ack {
                        block_num: block_num,
                    };

                    send_retry(&socket, None, &ack_packet.serialize()?, DEFAULT_RETRY_COUNT)?;

                    /* If data size is less than block size, terminate, else expect next block. */
                    if data.len() < DATA_BLOCK_SIZE {
                        break;
                    }

                    data_block_expected += 1;
                }

                /*
                * For block number greater than expected, the client probably lost some data from
                  server. Discard data, no need to ACK either. Just retrieve another packet and
                  work with it.
                */

                /* After handling all the cases, retrieve next packet from the server. */
                (rd_size, _) = recv_retry(&socket, &mut byte_arr, DEFAULT_RETRY_COUNT, true)?;
            }
            TftpPacket::Error {
                error_code,
                error_msg,
            } => {
                return Err(TftpError::ErrorPacket(
                    error_code,
                    if error_code == ERROR_CODE_SEE_MSG {
                        error_msg
                    } else {
                        error_msg_from_code(error_code)
                    },
                ));
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
    }

    /* Flush any buffered bytes that are yet to be written. */
    file_buf.flush().map_err(|err: Error| {
        _ = fs::remove_file(&temp_file_path);
        err.to_string()
    })?;

    /* Rename the file. */
    let dest_file_path: PathBuf = root_dir.as_ref().join(dest_filename);
    fs::rename(&temp_file_path, &dest_file_path)?;
    Ok(())
}
