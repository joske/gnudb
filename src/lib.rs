//! Crate to get CDDB information from gnudb.org (like cddb.com and freedb.org in the past)
//!
//! Right now only login, query and read are implemented, and only over CDDBP (not HTTP)
//! All I/O is now done async

use async_std::{
    io::{prelude::BufReadExt, BufReader, WriteExt},
    net::TcpStream,
};
use std::net::Shutdown;

use discid::DiscId;

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

#[derive(Debug)]
pub struct Connection {
    stream: TcpStream,
}

impl Connection {
    /// create a new connection to given host:port combination
    pub async fn from_host_port(host: &str, port: u16) -> Result<Connection, String> {
        let s = format!("{}:{}", host, port);
        connect(s).await
    }

    /// create a new connection to gnudb.gnudb.org port 8880
    pub async fn new() -> Result<Connection, String> {
        connect("gnudb.gnudb.org:8880".to_owned()).await
    }

    /// query gnudb for a given discid
    /// returns a vector of matches or an error
    pub async fn query(&mut self, discid: &DiscId) -> Result<Vec<Match>, String> {
        // the protocol is as follows:
        // cddb query discid ntrks off1 off2 ... nsecs

        // CDs don't *have* to start at track number 1...
        let count = discid.last_track_num() - discid.first_track_num() + 1;
        let mut toc = discid.toc_string();
        // the toc from DiscId is total_sectors first_track off1 off2 ... offn
        // so we take from the 3rd item in the toc
        let mut split = toc.splitn(4, ' ');
        toc = split.nth(3).unwrap().to_owned(); // this should be the rest of the string
        let query = format!(
            "cddb query {} {} {} {}\n",
            discid.freedb_id(),
            count,
            toc,
            discid.sectors() / 75
        );
        cddb_query(&mut self.stream, query).await
    }

    /// read all data of a given disc
    pub async fn read(&mut self, single_match: &Match) -> Result<Disc, String> {
        cddb_read(&mut self.stream, single_match).await
    }

    pub fn close(&mut self) {
        self.stream.shutdown(Shutdown::Both).unwrap();
    }
}

/// connect the tcp stream, login and set the protocol to 6
async fn connect(s: String) -> Result<Connection, String> {
    let stream = TcpStream::connect(s.clone()).await;
    if stream.is_ok() {
        let mut stream = stream.unwrap();
        println!("Successfully connected to server {}", s.clone());
        // say hello -> this is the login
        let mut hello = String::new();
        let mut reader = BufReader::new(stream.clone());
        reader.read_line(&mut hello).await.unwrap();
        let hello = "cddb hello ripperx localhost ripperx 4\n".to_owned();
        send_command(&mut stream, hello).await?;

        // switch to protocol level 6, so the output of GNUDB contains DYEAR and DGENRE
        let proto = "proto 6\n".to_owned();
        send_command(&mut stream, proto).await?;
        return Ok(Connection { stream });
    }
    Err(stream.err().unwrap().to_string())
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

/// specific command to query the disc, first issues a query, and then a read
/// query protocol: cddb query discid ntrks off1 off2 ... nsecs
/// if nothing found, will return empty matches
async fn cddb_query(stream: &mut TcpStream, cmd: String) -> Result<Vec<Match>, String> {
    let response = send_command(stream, cmd).await?;
    let mut matches: Vec<Match> = Vec::new();
    for line in response.lines() {
        if line.starts_with("200") {
            // exact match
            let mut split = line.splitn(4, ' ');
            let _code = split.next();
            let category = split.next().unwrap();
            let discid = split.next().unwrap();
            let remainder = split.next().unwrap();
            let mut split = remainder.split('/');
            let title = split.next().unwrap().trim();
            let artist = split.next().unwrap().trim();
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
        let m = parse_matches(line);
        matches.push(m);
    }
    Ok(matches)
}

/// specific command to read the disc
/// read protocol: cddb read category discid
async fn cddb_read(stream: &mut TcpStream, single_match: &Match) -> Result<Disc, String> {
    let cmd = format!(
        "cddb read {} {}\n",
        single_match.category, single_match.discid
    );
    let data = send_command(stream, cmd).await?;
    let disc = parse_data(data);
    println!("disc:{:?}", disc);
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
async fn send_command(stream: &mut TcpStream, cmd: String) -> Result<String, String> {
    let msg = cmd.as_bytes();
    stream.write(msg).await.unwrap();
    println!("sent {}", cmd);
    let mut response = String::new();
    let mut reader = BufReader::new(stream.clone());
    match reader.read_line(&mut response).await {
        Ok(_) => {
            print!("response: {}", response);
            if response.starts_with('5') {
                // eek!
                Err(response)
            } else {
                // ok, check second digit
                if response.chars().nth(1).unwrap() == '0' {
                    // no more lines
                    Ok(response)
                } else if response.chars().nth(1).unwrap() == '1'
                    || response.chars().nth(1).unwrap() == '2'
                {
                    // more lines to read
                    let mut data = String::new();
                    let mut response = String::new();
                    loop {
                        let result = reader.read_line(&mut response).await;
                        print!("response: {}", response);
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
                                println!("Failed to receive data: {}", e);
                                return Err("failed to read".to_owned());
                            }
                        }
                    }
                    Ok(data)
                } else {
                    Err(response)
                }
            }
        }
        Err(e) => {
            println!("Failed to send command: {}", e);
            Err(e.to_string())
        }
    }
}

/// parse a line of inexact matches
fn parse_matches(line: &str) -> Match {
    let mut split = line.splitn(3, ' ');
    let category = split.next().unwrap();
    let id = split.next().unwrap();
    let remainder = split.next().unwrap();
    let mut split = remainder.split('/');
    let title = split.next().unwrap().trim();
    let artist = split.next().unwrap().trim();
    Match {
        discid: id.to_owned(),
        category: category.to_owned(),
        title: title.to_owned(),
        artist: artist.to_owned(),
    }
}

/// parse the full response from the CDDB server
fn parse_data(data: String) -> Disc {
    println!("{}", data);
    let mut disc = Disc {
        ..Default::default()
    };
    let mut i = 0;
    for line in data.lines() {
        if line.starts_with("DTITLE") {
            let value = line.split('=').nth(1).unwrap();
            let mut split = value.split('/');
            disc.artist = split.next().unwrap().trim().to_owned();
            disc.title = split.next().unwrap().trim().to_owned();
        }
        if line.starts_with("DYEAR") {
            let value = line.split('=').nth(1).unwrap();
            disc.year = Some(value.parse::<u16>().unwrap());
        }
        if line.starts_with("DGENRE") {
            let value = line.split('=').nth(1).unwrap();
            disc.genre = Some(value.to_owned());
        }
        // since we use protocol level 6, we should get the year/genre via DYEAR and DGENRE, and these should come before EXTD
        // this is as a fallback
        if disc.year.is_none() && line.starts_with("EXTD") {
            let mut split = line.splitn(2, "YEAR:");
            let y = split.nth(1).unwrap();
            split = y.splitn(2, " ");
            disc.year = Some(split.next().unwrap().parse::<u16>().unwrap());
        }
        if line.starts_with("TTITLE") {
            let mut track = Track {
                ..Default::default()
            };
            track.number = i + 1; // tracks are 0 based in CDDB/GNUDB
            track.title = line.split('=').nth(1).unwrap().to_owned();
            track.artist = disc.artist.clone();
            disc.tracks.push(track);
            i += 1; // assume tracks are consecutive - this is not necessarily true
        }
    }
    disc
}

async fn _example() {
    // get a disc id by querying the disc in the default CD/DVD ROM drive
    let discid = DiscId::read(Some(DiscId::default_device().as_str())).unwrap();
    // open a connection
    let mut con = Connection::new().await.unwrap();
    // find a list of matches (could be multiple)
    let matches: Vec<Match> = con.query(&discid).await.unwrap();
    // select the right match
    let m: &Match = &matches[2];
    // read all the metadata
    let _disc = con.read(m).await.unwrap();
    // close the connection (Drop trait is implemented, so not strictly necessary)
    con.close();
}

#[cfg(test)]
mod test {
    use discid::DiscId;

    use crate::Connection;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    #[test]
    fn test_good_url() {
        let con = aw!(Connection::from_host_port("gnudb.gnudb.org", 8880));
        assert!(con.is_ok());
    }

    #[test]
    fn test_bad_url() {
        let con = aw!(Connection::from_host_port("localhost", 80));
        assert!(con.is_err());
    }

    #[test]
    fn test_search() {
        let offsets = [
            185700, 150, 18051, 42248, 57183, 75952, 89333, 114384, 142453, 163641,
        ];
        let discid = DiscId::put(1, &offsets).unwrap();
        println!("freedb {}", discid.freedb_id());
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
        let offsets = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
        let discid = DiscId::put(1, &offsets).unwrap();
        println!("freedb {}", discid.freedb_id());
        let mut con = aw!(Connection::new()).unwrap();
        let matches = aw!(con.query(&discid));
        assert!(matches.is_ok());
        let matches = matches.unwrap();
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_inexact_search() {
        let offsets = [
            185710, 150, 18075, 42275, 57184, 75952, 89333, 114386, 142451, 163642,
        ];
        let discid = DiscId::put(1, &offsets).unwrap();
        println!("freedb {}", discid.freedb_id());
        let mut con = aw!(Connection::new()).unwrap();
        let matches = aw!(con.query(&discid));
        assert!(matches.is_ok());
        let matches = matches.unwrap();
        assert_eq!(matches.len(), 13);
        let disc = aw!(con.read(&matches[2]));
        assert!(disc.is_ok());
        let disc = disc.unwrap();
        assert_eq!(disc.year.unwrap(), 1978);
        assert_eq!(disc.tracks.len(), 9);
        assert_eq!(disc.genre.unwrap(), "Rock");
        assert_eq!(disc.title, "Dire Straits");
        assert_eq!(disc.artist, "Dire Straits");
        assert_eq!(disc.year, Some(1978));
    }

    #[test]
    fn test_parse() {
        let input = r"# xmcd
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
PLAYORDER="
            .to_owned();
        let disc = super::parse_data(input);
        assert_eq!(disc.year.unwrap(), 2002);
        assert_eq!(disc.title, "(black) Mutter");
        assert_eq!(disc.tracks.len(), 11);
        assert_eq!(disc.genre.unwrap(), "Industrial Metal");
        assert_eq!(disc.year, Some(2002));
    }

    #[test]
    fn test_extd() {
        let input = r"# xmcd
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
PLAYORDER="
            .to_owned();
        let disc = super::parse_data(input);
        assert_eq!(disc.year.unwrap(), 1978);
        assert_eq!(disc.genre.unwrap(), "Rock");
        assert_eq!(disc.tracks.len(), 9);
        assert_eq!(disc.title, "Dire Straits");
        assert_eq!(disc.artist, "DIRE STRAITS");
        assert_eq!(disc.year, Some(1978));
    }
}
