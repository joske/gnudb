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
use smol::{io::BufReader, net::TcpStream};
use std::net::Shutdown;

use discid::DiscId;
use error::GnuDbError;

pub mod error;
mod cddbp;
mod http;
mod parser;

pub(crate) const HELLO_STRING: &str = "ripperx localhost ripperx 4";

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
    let cmd = parser::create_query_cmd(discid)?;
    let cmd = cmd.trim_end();
    let body = http::http_request(host, port, cmd)?;

    let data = parser::parse_raw_response(&body)?;
    debug!("HTTP response data:\n{}", data);
    parser::parse_query_response(data)
}

/// HTTP read to a GNUDb server to fetch a single disc's metadata
/// Every request creates a new connection
pub fn http_read(host: &str, port: u16, single_match: &Match) -> Result<Disc, GnuDbError> {
    let cmd = parser::create_read_cmd(single_match);
    let cmd = cmd.trim_end();
    let body = http::http_request(host, port, cmd)?;
    let disc = parser::parse_read_response(body)?;
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
        cddbp::connect(s).await
    }

    /// create a new connection to gnudb.gnudb.org port 8880
    pub async fn new() -> Result<Connection, GnuDbError> {
        cddbp::connect("gnudb.gnudb.org:8880".to_owned()).await
    }

    /// query gnudb for a given discid
    /// returns a vector of matches or an error
    pub async fn query(&mut self, discid: &DiscId) -> Result<Vec<Match>, GnuDbError> {
        let query = parser::create_query_cmd(discid)?;
        cddbp::cddb_query(&mut self.reader, query).await
    }

    /// read all data of a given disc
    pub async fn read(&mut self, single_match: &Match) -> Result<Disc, GnuDbError> {
        cddbp::cddb_read(&mut self.reader, single_match).await
    }

    pub fn close(&mut self) {
        self.reader.get_mut().shutdown(Shutdown::Both).ok();
    }

    pub(crate) fn from_reader(reader: BufReader<TcpStream>) -> Self {
        Connection { reader }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod test;
