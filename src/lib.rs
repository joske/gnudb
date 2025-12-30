//! Crate to get CDDB information from gnudb.org (like cddb.com and freedb.org in the past)
//!
//! Right now only login, query and read are implemented, and only over CDDBP (not HTTP)
//! All I/O is now done async using smol.
//!
//! Example usage:
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
use std::net::Shutdown;

use discid::DiscId;
use error::GnuDbError;

pub mod error;

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
        // the protocol is as follows:
        // cddb query discid ntrks off1 off2 ... nsecs

        // CDs don't *have* to start at track number 1...
        let count = discid.last_track_num() - discid.first_track_num() + 1;
        let mut toc = discid.toc_string();
        // the toc from DiscId is total_sectors first_track off1 off2 ... offn
        // so we take from the 3rd item in the toc
        let mut split = toc.splitn(4, ' ');
        toc = split
            .nth(3)
            .ok_or(GnuDbError::ProtocolError("failed to parse toc".to_string()))?
            .to_owned(); // this should be the rest of the string
        let query = format!(
            "cddb query {} {} {} {}\n",
            discid.freedb_id(),
            count,
            toc,
            discid.sectors() / 75
        );
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

/// connect the tcp stream, login and set the protocol to 6
async fn connect(s: String) -> Result<Connection, GnuDbError> {
    let stream = TcpStream::connect(s.clone()).await?;
    let mut reader = BufReader::new(stream);
    debug!("Successfully connected to server {}", s.clone());
    // say hello -> this is the login
    let mut hello = String::new();
    reader.read_line(&mut hello).await?;
    let hello = "cddb hello ripperx localhost ripperx 4\n".to_owned();
    send_command(&mut reader, hello).await?;

    // switch to protocol level 6, so the output of GNUDB contains DYEAR and DGENRE
    let proto = "proto 6\n".to_owned();
    send_command(&mut reader, proto).await?;
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
    let cmd = format!(
        "cddb read {} {}\n",
        single_match.category, single_match.discid
    );
    let data = send_command(reader, cmd).await?;
    let disc = parse_data(data)?;
    debug!("disc:{:?}", disc);
    Ok(disc)
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
    let msg = cmd.as_bytes();
    reader.get_mut().write_all(msg).await?;
    debug!("sent {}", cmd);
    let mut response = String::new();
    match reader.read_line(&mut response).await {
        Ok(_) => {
            debug!("response: {}", response);
            if response.starts_with('5') {
                // eek!
                Err(GnuDbError::ProtocolError(response))
            } else {
                // ok, check second digit
                if response.chars().nth(1).ok_or(GnuDbError::ProtocolError(
                    "failed to parse response code".to_string(),
                ))? == '0'
                {
                    // no more lines
                    Ok(response)
                } else if response.chars().nth(1).ok_or(GnuDbError::ProtocolError(
                    "failed to parse response code".to_string(),
                ))? == '1'
                    || response.chars().nth(1).ok_or(GnuDbError::ProtocolError(
                        "failed to parse response code".to_string(),
                    ))? == '2'
                {
                    // more lines to read
                    let mut data = String::new();
                    let mut response = String::new();
                    loop {
                        let result = reader.read_line(&mut response).await;
                        debug!("response: {}", response);
                        match result {
                            Ok(_) => {
                                if response.starts_with('.') {
                                    // done
                                    break;
                                } else {
                                    data.push_str(response.as_str());
                                    response = String::new();
                                }
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
                    Ok(data)
                } else {
                    Err(GnuDbError::ProtocolError(response))
                }
            }
        }
        Err(e) => {
            debug!("Failed to send command: {}", e);
            Err(GnuDbError::ProtocolError(e.to_string()))
        }
    }
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
fn parse_data(data: String) -> Result<Disc, GnuDbError> {
    debug!("{}", data);
    let mut disc = Disc {
        ..Default::default()
    };
    for line in data.lines() {
        if line.starts_with("DTITLE") {
            let value = line
                .strip_prefix("DTITLE=")
                .ok_or(GnuDbError::ProtocolError(
                    "failed to parse DTITLE".to_owned(),
                ))?;
            let mut split = value.splitn(2, '/');
            disc.artist = split
                .next()
                .ok_or(GnuDbError::ProtocolError(
                    "failed to parse DTITLE artist".to_owned(),
                ))?
                .trim()
                .to_owned();
            disc.title = split
                .next()
                .ok_or(GnuDbError::ProtocolError(
                    "failed to parse DTITLE title".to_owned(),
                ))?
                .trim()
                .to_owned();
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
        if line.starts_with("DGENRE") {
            let value = line
                .strip_prefix("DGENRE=")
                .ok_or(GnuDbError::ProtocolError(
                    "failed to parse DGENRE".to_owned(),
                ))?
                .trim();
            disc.genre = Some(value.to_owned());
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

    use crate::{Connection, error::GnuDbError};

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    fn init_logger() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    fn test_good_url() {
        init_logger();
        let con = aw!(Connection::from_host_port("gnudb.gnudb.org", 8880));
        assert!(con.is_ok());
    }

    #[test]
    fn test_bad_url() {
        init_logger();
        let con = aw!(Connection::from_host_port("localhost", 80));
        assert!(con.is_err());
    }

    #[test]
    fn test_search() {
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
    fn test_search_bad_discid() {
        init_logger();
        let offsets = [235823, 0, 0, 0, 0];
        let discid = DiscId::put(3, &offsets).unwrap();
        debug!("freedb {}", discid.freedb_id());
        let mut con = aw!(Connection::new()).unwrap();
        let matches = aw!(con.query(&discid));
        assert!(matches.is_ok());
        let matches = matches.unwrap();
        assert_eq!(matches.len(), 0);
    }

    #[test]
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

    #[test]
    fn test_parse() -> Result<(), GnuDbError> {
        init_logger();
        let disc = super::parse_data(RAMMSTEIN.to_string())?;
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
        let disc = super::parse_data(DIRE_STRAITS.to_string())?;
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
        let disc = super::parse_data(NO_YEAR.to_string())?;
        assert!(disc.year.is_none());
        assert_eq!(disc.title, "Mystery Record");
        assert_eq!(disc.artist, "Unknown Artist");
        Ok(())
    }

    #[test]
    fn test_invalid_dyear_uses_extd() -> Result<(), GnuDbError> {
        init_logger();
        let disc = super::parse_data(INVALID_DYEAR_EXTD.to_string())?;
        assert_eq!(disc.year, Some(1999));
        assert_eq!(disc.genre.as_deref(), Some("Alt"));
        Ok(())
    }

    #[test]
    fn test_valid_dyear_overrides_extd() -> Result<(), GnuDbError> {
        init_logger();
        let disc = super::parse_data(CONFLICTING_DYEAR.to_string())?;
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
}
