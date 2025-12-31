//! Crate to get CDDB information from gnudb.org (like cddb.com and freedb.org in the past)
//!
//! Right now only login, query and read are implemented, both over HTTP and CDDBP protocol.
//! All CDDBP I/O is done async using smol.
//! The HTTP functions are synchronous for simplicity, using ureq.
//!
//! Example HTTP usage:
//! ```no_run
//! use gnudb::{Connection, Match};
//! use discid::DiscId;
//! use gnudb::{http_query, http_read};
//!
//!     // get a disc id by querying the disc in the default CD/DVD ROM drive
//!     let discid = DiscId::read(Some(DiscId::default_device().as_str())).unwrap();
//!     let matches: Vec<Match> = http_query("gnudb.gnudb.org", 80, &discid).unwrap();
//!     // select the right match
//!     let m: &Match = &matches[2];
//!     // read all the metadata
//!     let _disc = http_read("gnudb.gnudb.org", 80, m).unwrap();
//!
//! ```
//!
//! Example CDDBP usage:
//! ```no_run
//! use gnudb::{Connection, Match};
//! use discid::DiscId;
//! use smol::block_on;
//!
//! block_on(async {
//!     // get a disc id by querying the disc in the default CD/DVD ROM drive
//!     let discid = DiscId::read(Some(DiscId::default_device().as_str())).unwrap();
//!     // open a connection
//!     let mut con = Connection::new().await.unwrap();
//!     // find a list of matches (could be multiple)
//!     let matches: Vec<Match> = con.query(&discid).await.unwrap();
//!     // select the right match
//!     let m: &Match = &matches[2];
//!     // read all the metadata
//!     let _disc = con.read(m).await.unwrap();
//!     // close the connection (Drop trait is implemented, so not strictly necessary)
//!     con.close();
//! });
//! ```

use log::debug;
use smol::{io::BufReader, net::TcpStream, prelude::*};
use std::{net::Shutdown, time::Duration};

use discid::DiscId;
use error::GnuDbError;

pub mod error;

const HELLO_STRING: &str = "ripperx localhost ripperx 4";
const PROTO_CMD: &str = "proto 6\n";
const HTTP_PATH: &str = "/~cddb/cddb.cgi";

#[derive(Default, Debug, Clone)]
pub struct Match {
    pub discid: String,
    pub category: String,
    pub artist: String,
    pub title: String,
}

#[derive(Default, Debug)]
pub struct Disc {
    pub title: String,
    pub artist: String,
    pub year: Option<u16>,
    pub genre: Option<String>,
    pub tracks: Vec<Track>,
}

#[derive(Default, Debug)]
pub struct Track {
    pub number: u32,
    pub title: String,
    pub artist: String,
    pub duration: u64,
    pub composer: Option<String>,
}

/// HTTP query to a GNUDb server for a given discid
/// returns a vector of matches or an error
/// Every query creates a new connection
pub fn http_query(host: &str, port: u16, discid: &DiscId) -> Result<Vec<Match>, GnuDbError> {
    let cmd = create_query_cmd(discid)?;
    let cmd = cmd.trim_end();
    let body = http_request(host, port, cmd)?;

    let data = parse_raw_response(&body)?;
    debug!("HTTP response data:\n{}", data);
    parse_query_response(data)
}

/// HTTP read to a GNUDb server to fetch a single disc's metadata
/// Every request creates a new connection
pub fn http_read(host: &str, port: u16, single_match: &Match) -> Result<Disc, GnuDbError> {
    let cmd = create_read_cmd(single_match);
    let cmd = cmd.trim_end();
    let body = http_request(host, port, cmd)?;
    let disc = parse_read_response(body)?;
    debug!("disc:{:?}", disc);
    Ok(disc)
}

/// Represents a CDDBP connection to a GNUDb server
/// Multiple commands can be sent over the same connection
pub struct Connection {
    reader: BufReader<TcpStream>,
}

impl Connection {
    /// create a new connection to given host:port combination
    pub async fn from_host_port(host: &str, port: u16) -> Result<Connection, GnuDbError> {
        let s = format!("{}:{}", host, port);
        connect(s).await
    }

    /// create a new connection to gnudb.gnudb.org port 8880
    pub async fn new() -> Result<Connection, GnuDbError> {
        connect("gnudb.gnudb.org:8880".to_owned()).await
    }

    /// query gnudb for a given discid
    /// returns a vector of matches or an error
    pub async fn query(&mut self, discid: &DiscId) -> Result<Vec<Match>, GnuDbError> {
        let query = create_query_cmd(discid)?;
        cddb_query(&mut self.reader, query).await
    }

    /// read all data of a given disc
    pub async fn read(&mut self, single_match: &Match) -> Result<Disc, GnuDbError> {
        cddb_read(&mut self.reader, single_match).await
    }

    pub fn close(&mut self) {
        self.reader.get_mut().shutdown(Shutdown::Both).ok();
    }
}

fn create_query_cmd(discid: &DiscId) -> Result<String, GnuDbError> {
    let count = discid.last_track_num() - discid.first_track_num() + 1;
    let mut toc = discid.toc_string();
    let mut split = toc.splitn(4, ' ');
    toc = split
        .nth(3)
        .ok_or(GnuDbError::ProtocolError("failed to parse toc".to_string()))?
        .to_owned();
    let query = format!(
        "cddb query {} {} {} {}\n",
        discid.freedb_id(),
        count,
        toc,
        discid.sectors() / 75
    );
    Ok(query)
}

fn http_request(host: &str, port: u16, cmd: &str) -> Result<String, GnuDbError> {
    let url = format!("http://{host}:{port}{HTTP_PATH}");
    debug!("HTTP request URL: {}", url);
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_body(Some(Duration::from_secs(10)))
        .build();
    let agent: ureq::Agent = config.into();
    let mut response = agent
        .get(&url)
        .query("cmd", cmd)
        .query("hello", HELLO_STRING)
        .query("proto", "6")
        .call()?;
    let body = response.body_mut().read_to_string()?;
    debug!("HTTP response body:\n{}", body);
    Ok(body)
}

/// connect the tcp stream, login and set the protocol to 6
async fn connect(s: String) -> Result<Connection, GnuDbError> {
    let stream = TcpStream::connect(&s).await?;
    let mut reader = BufReader::new(stream);
    debug!("Successfully connected to server {}", &s);
    // say hello -> this is the login
    let mut server_hello = String::new();
    reader.read_line(&mut server_hello).await?;
    let our_hello = format!("cddb hello {HELLO_STRING}\n");
    send_command(&mut reader, our_hello).await?;

    // switch to protocol level 6, so the output of GNUDB contains DYEAR and DGENRE
    send_command(&mut reader, PROTO_CMD.to_owned()).await?;
    Ok(Connection { reader })
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

/// specific command to query the disc, first issues a query, and then a read
/// query protocol: cddb query discid ntrks off1 off2 ... nsecs
/// if nothing found, will return empty matches
async fn cddb_query(
    reader: &mut BufReader<TcpStream>,
    cmd: String,
) -> Result<Vec<Match>, GnuDbError> {
    let response = send_command(reader, cmd).await?;
    let matches = parse_query_response(response)?;
    Ok(matches)
}

fn parse_query_response(response: String) -> Result<Vec<Match>, GnuDbError> {
    let mut matches: Vec<Match> = Vec::new();
    for line in response.lines() {
        if line.starts_with("200") {
            // exact match
            let mut split = line.splitn(4, ' ');
            let _code = split.next();
            let category = split.next().ok_or(GnuDbError::ProtocolError(
                "failed to parse exact match category".to_owned(),
            ))?;
            let discid = split.next().ok_or(GnuDbError::ProtocolError(
                "failed to parse exact match discid".to_owned(),
            ))?;
            let remainder = split.next().ok_or(GnuDbError::ProtocolError(
                "failed to parse exact match remainder".to_owned(),
            ))?;
            let mut split = remainder.split('/');
            let artist = split
                .next()
                .ok_or(GnuDbError::ProtocolError(
                    "failed to get artist".to_string(),
                ))?
                .trim();
            let title = split
                .next()
                .ok_or(GnuDbError::ProtocolError("failed to get title".to_string()))?
                .trim();
            let m = Match {
                discid: discid.to_owned(),
                category: category.to_owned(),
                title: title.to_owned(),
                artist: artist.to_owned(),
            };
            matches.push(m);
            break;
        }
        if line.starts_with("202") {
            // no matches
            break;
        }
        if line.starts_with("211") {
            continue; // ignore first status line
        }
        let m = parse_matches(line)?;
        matches.push(m);
    }
    Ok(matches)
}

/// specific command to read the disc
/// read protocol: cddb read category discid
async fn cddb_read(
    reader: &mut BufReader<TcpStream>,
    single_match: &Match,
) -> Result<Disc, GnuDbError> {
    let cmd = create_read_cmd(single_match);
    let data = send_command(reader, cmd).await?;
    let disc = parse_read_response(data)?;
    debug!("disc:{:?}", disc);
    Ok(disc)
}

fn create_read_cmd(single_match: &Match) -> String {
    format!(
        "cddb read {} {}\n",
        single_match.category, single_match.discid
    )
}

/// send a CDDBP command, and parse its output, according to the protocol specs:
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
    debug!("sent {}", cmd);
    let mut status = String::new();
    reader.read_line(&mut status).await?;
    debug!("response: {}", status);

    let second_digit = status.chars().nth(1).ok_or(GnuDbError::ProtocolError(
        "failed to parse response code".to_string(),
    ))?;
    let mut raw = status.clone();

    if second_digit == '1' || second_digit == '2' {
        loop {
            let mut line = String::new();
            let result = reader.read_line(&mut line).await;
            debug!("response: {}", line);
            match result {
                Ok(_) => {
                    if line.trim_end_matches(['\r', '\n']).eq(".") {
                        break;
                    }
                    raw.push_str(&line);
                }
                Err(e) => {
                    debug!("Failed to receive data: {}", e);
                    return Err(GnuDbError::ProtocolError(format!(
                        "failed to read line: {}",
                        e
                    )));
                }
            }
        }
    }

    Ok(raw)
}

/// parse the raw response from the server according to the protocol
fn parse_raw_response(raw: &str) -> Result<String, GnuDbError> {
    let (status, rest) = match raw.split_once('\n') {
        Some((head, tail)) => (head, Some(tail)),
        None => (raw, None),
    };

    if status.starts_with('4') || status.starts_with('5') {
        return Err(GnuDbError::ProtocolError(status.to_string()));
    }

    let second_digit = status.chars().nth(1).ok_or(GnuDbError::ProtocolError(
        "failed to parse response code".to_string(),
    ))?;
    if second_digit == '0' {
        if rest.is_some() {
            return Ok(format!("{}\n", status));
        }
        return Ok(status.to_string());
    }

    if second_digit != '1' && second_digit != '2' {
        return Err(GnuDbError::ProtocolError(status.to_string()));
    }

    let mut data = String::new();
    if let Some(rest) = rest {
        for line in rest.lines() {
            if line == "." {
                break;
            }
            if let Some(stripped) = line.strip_prefix("..") {
                data.push('.');
                data.push_str(stripped);
            } else {
                data.push_str(line);
            }
            data.push('\n');
        }
    }

    Ok(data)
}

/// parse a line of inexact matches
fn parse_matches(line: &str) -> Result<Match, GnuDbError> {
    let mut split = line.splitn(3, ' ');
    let category = split.next().ok_or(GnuDbError::ProtocolError(
        "failed to parse category".to_owned(),
    ))?;
    let id = split
        .next()
        .ok_or(GnuDbError::ProtocolError("failed to parse id".to_owned()))?;
    let remainder = split.next().ok_or(GnuDbError::ProtocolError(
        "failed to parse remainder".to_owned(),
    ))?;
    let mut split = remainder.split('/');
    let artist = split
        .next()
        .ok_or(GnuDbError::ProtocolError(
            "failed to parse artist".to_string(),
        ))?
        .trim();
    let title = split
        .next()
        .ok_or(GnuDbError::ProtocolError(
            "failed to parse title".to_string(),
        ))?
        .trim();
    Ok(Match {
        discid: id.to_owned(),
        category: category.to_owned(),
        title: title.to_owned(),
        artist: artist.to_owned(),
    })
}

/// parse the full response from the CDDB server
fn parse_read_response(data: String) -> Result<Disc, GnuDbError> {
    debug!("{}", data);
    let mut disc = Disc {
        ..Default::default()
    };
    for line in data.lines() {
        if let Some(value) = line.strip_prefix("DTITLE=") {
            let mut split = value.splitn(2, '/');
            let first = split.next().unwrap_or("").trim();
            if let Some(rest) = split.next() {
                disc.artist = first.to_owned();
                disc.title = rest.trim().to_owned();
            } else {
                disc.title = first.to_owned();
            }
        }
        if let Some(value) = line.strip_prefix("DYEAR=") {
            let value = value.trim();
            if !value.is_empty() {
                match value.parse::<u16>() {
                    Ok(year) => disc.year = Some(year),
                    Err(e) => {
                        debug!("failed to parse DYEAR '{}': {}", value, e);
                    }
                }
            }
        }
        if let Some(value) = line.strip_prefix("DGENRE=") {
            let value = value.trim();
            if !value.is_empty() {
                disc.genre = Some(value.to_owned());
            }
        }
        // since we use protocol level 6, we should get the year/genre via DYEAR and DGENRE, and these should come before EXTD
        // this is as a fallback
        if disc.year.is_none()
            && line.starts_with("EXTD")
            && let Some(pos) = line.find("YEAR:")
        {
            let value = line[(pos + "YEAR:".len())..]
                .split_whitespace()
                .next()
                .ok_or(GnuDbError::ProtocolError(
                    "failed to parse EXTD YEAR".to_owned(),
                ))?;
            disc.year = Some(value.parse::<u16>().map_err(|e| {
                GnuDbError::ProtocolError(format!("failed to parse EXTD YEAR: {}", e))
            })?);
        }

        if line.starts_with("TTITLE") {
            let rest = line
                .strip_prefix("TTITLE")
                .ok_or(GnuDbError::ProtocolError(
                    "failed to parse TTITLE".to_owned(),
                ))?;
            let (index_str, title) = rest.split_once('=').ok_or(GnuDbError::ProtocolError(
                "failed to parse TTITLE value".to_owned(),
            ))?;
            let index = index_str.trim().parse::<u32>().map_err(|e| {
                GnuDbError::ProtocolError(format!("failed to parse TTITLE index: {}", e))
            })?;
            let mut track = Track {
                ..Default::default()
            };
            track.number = index + 1; // tracks are 0 based in CDDB/GNUDB
            track.title = title.to_owned();
            track.artist = disc.artist.clone();
            disc.tracks.push(track);
        }
    }
    Ok(disc)
}

// run these tests with `--test-threads=1` or you'll get hangs because gnudb doesn't like multiple connections from same IP
#[cfg(test)]
mod test {
    use discid::DiscId;
    use log::debug;
    use serial_test::serial;

    use crate::{Connection, error::GnuDbError, http_query, http_read};

    macro_rules! aw {
        ($e:expr) => {
            smol::block_on($e)
        };
    }

    fn init_logger() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    #[serial]
    fn test_good_url() {
        init_logger();
        let con = aw!(Connection::from_host_port("gnudb.gnudb.org", 8880));
        assert!(con.is_ok());
    }

    #[test]
    #[serial]
    fn test_bad_url() {
        init_logger();
        let con = aw!(Connection::from_host_port("localhost", 80));
        assert!(con.is_err());
    }

    #[test]
    #[serial]
    fn test_http_exact_search() {
        init_logger();
        let offsets = [
            185700, 150, 18051, 42248, 57183, 75952, 89333, 114384, 142453, 163641,
        ];
        let discid = DiscId::put(1, &offsets).unwrap();
        debug!("freedb {}", discid.freedb_id());
        let matches = http_query("gnudb.gnudb.org", 80, &discid);
        assert!(matches.is_ok());
        let matches = matches.unwrap();
        assert_eq!(matches.len(), 1);
        let disc = http_read("gnudb.gnudb.org", 80, &matches[0]);
        assert!(disc.is_ok());
        let disc = disc.unwrap();
        assert_eq!(disc.year.unwrap(), 1978);
        assert_eq!(disc.tracks.len(), 9);
        assert_eq!(disc.genre.unwrap(), "Rock");
        assert_eq!(disc.title, "Dire Straits");
        assert_eq!(disc.artist, "DIRE STRAITS");
        assert_eq!(disc.year, Some(1978));
    }

    #[test]
    #[serial]
    fn test_exact_search() {
        init_logger();
        let offsets = [
            185700, 150, 18051, 42248, 57183, 75952, 89333, 114384, 142453, 163641,
        ];
        let discid = DiscId::put(1, &offsets).unwrap();
        debug!("freedb {}", discid.freedb_id());
        let mut con = aw!(Connection::new()).unwrap();
        let matches = aw!(con.query(&discid));
        assert!(matches.is_ok());
        let matches = matches.unwrap();
        assert_eq!(matches.len(), 1);
        let disc = aw!(con.read(&matches[0]));
        assert!(disc.is_ok());
        let disc = disc.unwrap();
        assert_eq!(disc.year.unwrap(), 1978);
        assert_eq!(disc.tracks.len(), 9);
        assert_eq!(disc.genre.unwrap(), "Rock");
        assert_eq!(disc.title, "Dire Straits");
        assert_eq!(disc.artist, "DIRE STRAITS");
        assert_eq!(disc.year, Some(1978));
    }

    #[test]
    #[serial]
    fn test_inexact_search() {
        init_logger();
        let offsets = [
            185710, 150, 18025, 42275, 57184, 75952, 89333, 114386, 142451, 163695,
        ];
        let discid = DiscId::put(1, &offsets).unwrap();
        debug!("freedb {}", discid.freedb_id());
        let mut con = aw!(Connection::new()).unwrap();
        let matches = aw!(con.query(&discid));
        assert!(matches.is_ok());
        let matches = matches.unwrap();
        assert_eq!(matches.len(), 1);
        let disc = aw!(con.read(&matches[0]));
        assert!(disc.is_ok());
        let disc = disc.unwrap();
        assert_eq!(disc.year.unwrap(), 1978);
        assert_eq!(disc.tracks.len(), 9);
        assert_eq!(disc.genre.unwrap(), "Rock");
        assert_eq!(disc.title, "Dire Straits");
        assert_eq!(disc.artist, "DIRE STRAITS");
        assert_eq!(disc.year, Some(1978));
    }

    // Tests below don't require a network connection and so can be run in parallel
    #[test]
    fn test_parse_response_multiline_dotstuff() -> Result<(), GnuDbError> {
        let raw = "211 multiple matches\n..hello\nworld\n.\n";
        let data = super::parse_raw_response(raw)?;
        assert_eq!(data, ".hello\nworld\n");
        Ok(())
    }

    #[test]
    fn test_parse_response_single_line() -> Result<(), GnuDbError> {
        let raw = "200 OK\n";
        let data = super::parse_raw_response(raw)?;
        assert_eq!(data, "200 OK\n");
        Ok(())
    }

    #[test]
    fn test_parse_response_error() {
        let raw = "500 fail\n";
        let err = super::parse_raw_response(raw).unwrap_err();
        match err {
            GnuDbError::ProtocolError(_) => {}
            _ => panic!("unexpected error type"),
        }
    }

    #[test]
    fn test_no_match() {
        init_logger();
        let matches = super::parse_query_response(NO_MATCH.to_string());
        assert!(matches.is_ok());
        let matches = matches.unwrap();
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_parse() -> Result<(), GnuDbError> {
        init_logger();
        let disc = super::parse_read_response(RAMMSTEIN.to_string())?;
        assert_eq!(disc.year.unwrap(), 2002);
        assert_eq!(disc.title, "(black) Mutter");
        assert_eq!(disc.tracks.len(), 11);
        assert_eq!(disc.genre.unwrap(), "Industrial Metal");
        assert_eq!(disc.year, Some(2002));
        Ok(())
    }

    #[test]
    fn test_extd() -> Result<(), GnuDbError> {
        init_logger();
        let disc = super::parse_read_response(DIRE_STRAITS.to_string())?;
        assert_eq!(disc.year.unwrap(), 1978);
        assert_eq!(disc.genre.unwrap(), "Rock");
        assert_eq!(disc.tracks.len(), 9);
        assert_eq!(disc.title, "Dire Straits");
        assert_eq!(disc.artist, "DIRE STRAITS");
        assert_eq!(disc.year, Some(1978));
        Ok(())
    }

    #[test]
    fn test_missing_dyear() -> Result<(), GnuDbError> {
        init_logger();
        let disc = super::parse_read_response(NO_YEAR.to_string())?;
        assert!(disc.year.is_none());
        assert_eq!(disc.title, "Mystery Record");
        assert_eq!(disc.artist, "Unknown Artist");
        Ok(())
    }

    #[test]
    fn test_invalid_dyear_uses_extd() -> Result<(), GnuDbError> {
        init_logger();
        let disc = super::parse_read_response(INVALID_DYEAR_EXTD.to_string())?;
        assert_eq!(disc.year, Some(1999));
        assert_eq!(disc.genre.as_deref(), Some("Alt"));
        Ok(())
    }

    #[test]
    fn test_valid_dyear_overrides_extd() -> Result<(), GnuDbError> {
        init_logger();
        let disc = super::parse_read_response(CONFLICTING_DYEAR.to_string())?;
        assert_eq!(disc.year, Some(2001));
        Ok(())
    }

    #[test]
    fn test_parse_matches_artist_title() -> Result<(), GnuDbError> {
        init_logger();
        let m = super::parse_matches("rock abc123 ARTIST / Recording Title")?;
        assert_eq!(m.artist, "ARTIST");
        assert_eq!(m.title, "Recording Title");
        assert_eq!(m.category, "rock");
        assert_eq!(m.discid, "abc123");
        Ok(())
    }

    #[test]
    fn test_tracks_inherit_artist_and_numbering() -> Result<(), GnuDbError> {
        init_logger();
        let disc = super::parse_read_response(TWO_TRACKS.to_string())?;
        assert_eq!(disc.artist, "Sample Artist");
        assert_eq!(disc.title, "Example Album");
        assert_eq!(disc.genre, None);
        assert_eq!(disc.year, Some(1995));
        assert_eq!(disc.tracks.len(), 2);
        assert_eq!(disc.tracks[0].number, 1);
        assert_eq!(disc.tracks[0].title, "Track Zero");
        assert_eq!(disc.tracks[0].artist, "Sample Artist");
        assert_eq!(disc.tracks[1].number, 2);
        assert_eq!(disc.tracks[1].title, "Track One");
        assert_eq!(disc.tracks[1].artist, "Sample Artist");
        Ok(())
    }

    const NO_MATCH: &str = "202 No match for disc ID 000c4804.";

    const RAMMSTEIN: &str = r"# xmcd
#
# Track frame offsets:
#    150
#    25075
#    46501
#    70596
#    88533
#    105910
#    125169
#    147365
#    162906
#    190441
#    215174
#
# Disc length: 3186 seconds
#
# Revision: 2
# Processed by: cddbd v1.5.1PL2 Copyright (c) Steve Scherf et al.
# Submitted via: audiograbber 1.83.01
#
DISCID=940c700b
DTITLE=Rammstein+Sixtynine / (black) Mutter
DYEAR=2002
DGENRE=Industrial Metal
TTITLE0=Mein Herz Brennt (Nun Liebe Kinder Mix)
TTITLE1=Links 234 (Zwei Drei Vier Mix)
TTITLE2=Sonne (Laut Bis Zehn Mix)
TTITLE3=Ich Will (Ich Will Mix)
TTITLE4=Feuer Frei (Bang Bang Mein Ungluck Mix)
TTITLE5=Mutter (Violin Mix)
TTITLE6=Spieluhr (Ein Kleiner Mensch  Mix)
TTITLE7=Zwitter (Zwitter Zwitter Mix)
TTITLE8=Rein Raus (Raus Motherfucker Rein Mix)
TTITLE9=Adios (Er Hat Die Augen Aufgemacht Mix)
TTITLE10=Nebel (Eng Umschlungen Mix)
EXTD=
EXTT0=
EXTT1=
EXTT2=
EXTT3=
EXTT4=
EXTT5=
EXTT6=
EXTT7=
EXTT8=
EXTT9=
EXTT10=
PLAYORDER=";

    const DIRE_STRAITS: &str = r"# xmcd
#
# Track frame offsets:
#    150
#    18051
#    42248
#    57183
#    75952
#    89333
#    114384
#    142453
#    163641
#
# Disc length: 2476 seconds
#
# Revision: 7
# Processed by: cddbd v1.4PL0 Copyright (c) Steve Scherf et al.
# Submitted via: EasyCDDAExtractor 5.1.0
#
DISCID=6909aa09
DTITLE=DIRE STRAITS / Dire Straits
DYEAR=1978
DGENRE=Rock
TTITLE0=Down to the waterline
TTITLE1=Water of love
TTITLE2=Setting me up
TTITLE3=Six blade knife
TTITLE4=Southbound again
TTITLE5=Sultans of swing
TTITLE6=In the gallery
TTITLE7=Wild west end
TTITLE8=Lions
EXTD= YEAR: 1978 ID3G: 17
EXTT0=
EXTT1=
EXTT2=
EXTT3=
EXTT4=
EXTT5=
EXTT6=
EXTT7=
EXTT8=
PLAYORDER=";

    const NO_YEAR: &str = r"# xmcd
#
DTITLE=Unknown Artist / Mystery Record
DYEAR=
DGENRE=Unknown
TTITLE0=Track 01
";

    const INVALID_DYEAR_EXTD: &str = r"# xmcd
#
DTITLE=Sample Artist / Sample Title
DYEAR=abcd
DGENRE=Alt
TTITLE0=Track 01
EXTD= YEAR: 1999
";

    const CONFLICTING_DYEAR: &str = r"# xmcd
#
DTITLE=Artist / Title
DYEAR=2001
DGENRE=Rock
TTITLE0=Song
EXTD= YEAR: 1980
";

    const TWO_TRACKS: &str = r"# xmcd
#
DTITLE=Sample Artist / Example Album
DYEAR=
TTITLE0=Track Zero
TTITLE1=Track One
EXTD= YEAR: 1995
";
}
