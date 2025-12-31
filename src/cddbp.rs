use log::debug;
use smol::{Timer, io::BufReader, net::TcpStream, prelude::*};

use crate::error::GnuDbError;
use crate::parser::{
    create_read_cmd, parse_query_response, parse_raw_response, parse_read_response,
};
use crate::{Connection, Disc, HELLO_STRING, Match, TIMEOUT};

const PROTO_CMD: &str = "proto 6\n";

/// connect the tcp stream, login and set the protocol to 6
pub(crate) async fn connect(s: String) -> Result<Connection, GnuDbError> {
    let stream = TcpStream::connect(&s)
        .or(async {
            Timer::after(TIMEOUT).await;
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connection timed out",
            ))
        })
        .await?;
    let mut reader = BufReader::new(stream);
    debug!("Successfully connected to server {}", &s);
    // say hello -> this is the login
    let mut server_hello = String::new();
    read_line_with_timeout(&mut reader, &mut server_hello).await?;
    let our_hello = format!("cddb hello {HELLO_STRING}\n");
    send_command(&mut reader, our_hello).await?;

    // switch to protocol level 6, so the output of GNUDB contains DYEAR and DGENRE
    send_command(&mut reader, PROTO_CMD.to_owned()).await?;
    Ok(Connection::from_reader(reader))
}

/// specific command to query the disc, first issues a query, and then a read
/// query protocol: cddb query discid ntrks off1 off2 ... nsecs
/// if nothing found, will return empty matches
pub(crate) async fn cddb_query(
    reader: &mut BufReader<TcpStream>,
    cmd: String,
) -> Result<Vec<Match>, GnuDbError> {
    let response = send_command(reader, cmd).await?;
    let matches = parse_query_response(&response)?;
    Ok(matches)
}

/// specific command to read the disc
/// read protocol: cddb read category discid
pub(crate) async fn cddb_read(
    reader: &mut BufReader<TcpStream>,
    single_match: &Match,
) -> Result<Disc, GnuDbError> {
    let cmd = create_read_cmd(single_match);
    let data = send_command(reader, cmd).await?;
    let disc = parse_read_response(&data)?;
    debug!("disc:{disc:?}");
    Ok(disc)
}

/// send a CDDBP command, and parse its output, according to the protocol specs:
///
/// Server response code (three digit code):
///
/// First digit:
/// 1xx    Informative message
/// 2xx    Command OK
/// 3xx    Command OK so far, continue
/// 4xx    Command OK, but cannot be performed for some specified reasons
/// 5xx    Command unimplemented, incorrect, or program error
///
/// Second digit:
/// x0x    Ready for further commands
/// x1x    More server-to-client output follows (until terminating marker)
/// x2x    More client-to-server input follows (until terminating marker)
/// x3x    Connection will close
///
/// Third digit:
/// xx[0-9]    Command-specific code
async fn send_command(
    reader: &mut BufReader<TcpStream>,
    cmd: String,
) -> Result<String, GnuDbError> {
    let raw = read_response(reader, &cmd).await?;
    parse_raw_response(&raw)
}

async fn read_response(reader: &mut BufReader<TcpStream>, cmd: &str) -> Result<String, GnuDbError> {
    reader.get_mut().write_all(cmd.as_bytes()).await?;
    debug!("sent {cmd}");
    let mut status = String::new();
    read_line_with_timeout(reader, &mut status).await?;
    debug!("response: {status}");

    let second_digit = status.chars().nth(1).ok_or(GnuDbError::ProtocolError(
        "failed to parse response code".to_string(),
    ))?;
    let mut raw = status.clone();

    if second_digit == '1' || second_digit == '2' {
        loop {
            let mut line = String::new();
            let result = read_line_with_timeout(reader, &mut line).await;
            debug!("response: {line}");
            match result {
                Ok(_) => {
                    if line.trim_end_matches(['\r', '\n']).eq(".") {
                        break;
                    }
                    raw.push_str(&line);
                }
                Err(e) => {
                    debug!("Failed to receive data: {e}");
                    return Err(GnuDbError::ProtocolError(format!(
                        "failed to read line: {e}"
                    )));
                }
            }
        }
    }

    Ok(raw)
}

async fn read_line_with_timeout(
    reader: &mut BufReader<TcpStream>,
    buf: &mut String,
) -> Result<usize, GnuDbError> {
    let read = reader.read_line(buf);
    let timeout = async {
        Timer::after(TIMEOUT).await;
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "read timed out",
        ))
    };
    smol::future::or(read, timeout)
        .await
        .map_err(GnuDbError::from)
}
