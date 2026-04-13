/*
 * This file provides the main function for "tftpcl", the tftp client.
 */

use std::{
    env::{self, args},
    net::SocketAddr,
    process::exit,
};

use tftp::{
    core::TftpError,
    elog, elog_fatal,
    tftpcl_util::{TftpAction, get_file, help_info_tftpcl, parse_args, put_file},
};

fn main() {
    /*
     * Format for tftpcl: tftpcl [get|put] [filename] [server_address_ipv4]
     */

    let arg_list: Vec<String> = args().collect();

    let (action, src_filename, dest_filename, server_addr): (
        TftpAction,
        String,
        String,
        SocketAddr,
    ) = match parse_args(&arg_list[1..]) {
        Err(msg) => {
            elog_fatal!(-1, "{}", msg);
        }
        Ok(Some(result)) => result,
        Ok(None) => {
            /* If this return, someone just wanted to read help page. Terminate the program. */
            help_info_tftpcl();
            return;
        }
    };

    println!(
        "Action: {:?}, Source filename: {}, Destination filename: {}, Address: {}",
        action, src_filename, dest_filename, server_addr
    );

    let result: Result<(), TftpError> = match action {
        TftpAction::Get => get_file(src_filename, dest_filename, server_addr),
        TftpAction::Put => put_file(src_filename, dest_filename, server_addr),
    };

    match result {
        Err(err) => {
            elog_fatal!(-1, "{}", err);
        }
        _ => {}
    }
}
