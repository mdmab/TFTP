use std::{
    net::UdpSocket,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::core::{
    DEFAULT_RETRY_COUNT, ERROR_CODE_ACCESS_VIOLATION, TftpError, TftpPacket, error_msg_from_code,
    is_valid_filename, send_retry, transmission,
};

pub fn parse_args(args: &[String]) -> Result<Option<PathBuf>, String> {
    /* "args" is the list of arguments after the program name. */
    /* If no arguments are provided, show the help page instead. */
    if args.len() == 0 {
        return Ok(None);
    }

    /* Only 1 argument is expected, which is the root folder of the server. */
    let path: &Path = Path::new(&args[0]);
    let mut err_list: Vec<String> = Vec::new();

    if !path.is_dir() {
        err_list.push("The directory name doesn't point to a valid directory.".to_owned());
    }

    /* Existence of any other argument is an error. */
    for arg in &args[1..] {
        err_list.push(format!("Unknown argument: {}", arg));
    }

    if !err_list.is_empty() {
        return Err(err_list.join("\n"));
    }

    Ok(Some(path.to_owned()))
}

pub fn send_file(
    root_dir: Arc<PathBuf>,
    filename: &str,
    mode: &str,
    socket: &UdpSocket,
) -> Result<(), TftpError> {
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

        send_retry(socket, None, &err_pkt.to_byte_array()?, 1)?;
        return Err(err_msg.into());
    }

    /* RRQ request has been handled. Start sending data blocks. */
    let mut total_size: usize = 0;

    println!("\nReceiver address: {}", socket.peer_addr()?);

    transmission::send_file(
        root_dir,
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

pub fn receive_file(
    root_dir: Arc<PathBuf>,
    filename: &str,
    mode: &str,
    socket: &UdpSocket,
) -> Result<(), TftpError> {
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

        send_retry(socket, None, &err_pkt.to_byte_array()?, 1)?;
        return Err(err_msg.into());
    }

    /* WRQ request has been handled. Send an ACK packet with data block set to 0. */
    let ack_packet: TftpPacket = TftpPacket::Ack { block_num: 0 };
    send_retry(
        socket,
        None,
        &ack_packet.to_byte_array()?,
        DEFAULT_RETRY_COUNT,
    )?;

    /* Start receiving file. */
    let mut total_size: usize = 0;

    println!("\nSender address: {}", socket.peer_addr()?);

    transmission::receive_file(
        root_dir,
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

pub fn help_info_tftpd() {
    println!(
        "\
        \"tftpd\" TFTP server command line utility.\n\
        Syntax:  tftpcl [root_dir]\
        "
    );
}
