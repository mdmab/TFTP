use std::{
    fs::{File, OpenOptions},
    io::{BufReader, Error, ErrorKind, Read, Write},
    net::{SocketAddr, UdpSocket},
};

use crate::{
    core::{
        DATA_BLOCK_SIZE, DEFAULT_RETRY_COUNT, ERROR_CODE_ACCESS_VIOLATION,
        ERROR_CODE_FILE_NOT_FOUND, ERROR_CODE_ILLEGAL_OP, ERROR_CODE_SEE_MSG, ETH_FRAME_LEN,
        INVALID_DATA_ERROR, OPCODE_ACK, OPCODE_DATA, OPCODE_ERROR, OPCODE_RRQ, OPCODE_WRQ,
        TftpError, TftpPacket, error_msg_from_code, is_valid_filename, opcode_from_raw_data,
        recv_retry, send_retry, transmission,
    },
    elog,
};

pub fn send_file(filename: &str, mode: &str, socket: &UdpSocket) -> Result<(), TftpError> {
    /*
    * Check the validity of file name. If not valid, send an ERROR packet for access violation
      and terminate.
    */
    if !is_valid_filename(filename) {
        let (err_code, err_msg): (u16, String) = (
            ERROR_CODE_ACCESS_VIOLATION,
            error_msg_from_code(ERROR_CODE_ACCESS_VIOLATION),
        );

        let err_pkt: TftpPacket = TftpPacket::Error {
            error_code: err_code,
            error_msg: err_msg.clone(),
        };

        send_retry(socket, None, &err_pkt.serialize()?, 1)?;
        return Err(err_msg.into());
    }

    /* RRQ request has been handled. Start sending data blocks. */
    let mut total_size: usize = 0;

    println!("\nReceiver address: {}", socket.peer_addr()?);

    transmission::send_file(
        filename,
        mode,
        socket,
        |block_num: u16, block_size: usize| {
            total_size += block_size;

            if block_num > 1 {
                /* Move cursor 2 lines up, to refresh the contents. */
                print!("\x1b[2A");
            }

            println!(
                "Sent block: {}\nSent size: {} byte(s)",
                block_num, total_size
            );
        },
    )
}

pub fn receive_file(filename: &str, mode: &str, socket: &UdpSocket) -> Result<(), TftpError> {
    /*
    * Check the validity of file name. If not valid, send an ERROR packet for access violation
      and terminate.
    */
    if !is_valid_filename(filename) {
        let (err_code, err_msg): (u16, String) = (
            ERROR_CODE_ACCESS_VIOLATION,
            error_msg_from_code(ERROR_CODE_ACCESS_VIOLATION),
        );

        let err_pkt: TftpPacket = TftpPacket::Error {
            error_code: err_code,
            error_msg: err_msg.clone(),
        };

        send_retry(socket, None, &err_pkt.serialize()?, 1)?;
        return Err(err_msg.into());
    }

    /* WRQ request has been handled. Send an ACK packet with data block set to 0. */
    let ack_packet: TftpPacket = TftpPacket::Ack { block_num: 0 };
    send_retry(socket, None, &ack_packet.serialize()?, DEFAULT_RETRY_COUNT)?;

    /* Start receiving file. */
    let mut total_size: usize = 0;

    println!("\nSender address: {}", socket.peer_addr()?);

    transmission::receive_file(
        filename,
        mode,
        socket,
        None,
        |block_num: u16, block_size: usize| {
            total_size += block_size;

            if block_num > 1 {
                /* Move cursor 2 lines up, to refresh the contents. */
                print!("\x1b[2A");
            }

            println!(
                "Received block: {}\nReceived size: {} byte(s)",
                block_num, total_size
            );
        },
    )
}
