# TFTP

Implementation of basic TFTP client and server, based on RFC 1350. This project
consists of 2 separate executables: `tftpcl` (TFTP client) and `tftpd` (TFTP
server).

# Requirement

- Rust `>= 1.87.0`

# Building

To build both client `tftpcl` and server `tftpd`, run the following command:

```bash
cargo build --release
```

**Note**: This will also create `libtftp.rlib`, the library containing common
dependencies for both.

To build `tftpcl` or `tftpd` independently:

```bash
# This will create "tftpcl" only.
cargo build --release --bin tftpcl

# Similarly, this will create "tftpd" only.
cargo build --release --bin tftpd
```
