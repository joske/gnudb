//! Crate to get CDDB information from gnudb.org (like cddb.com and freedb.org in the past)
//!
//! Right now only login, query and read are implemented, and only over CDDBP (not HTTP)

use std::{
    io::{BufRead, BufReader, Write},
    net::{Shutdown, TcpStream},
};

use discid::DiscId;

#[derive(Debug)]
pub struct Connection {
    stream: TcpStream,
    logged_in : bool,
}

impl Connection {
    /// create a new connection to given host:port combination
    pub fn from_host_port(host: &str, port: u16) -> Self {
        let s = format!("{}:{}", host, port);
        Connection {
            stream: TcpStream::connect(s).unwrap(),
            logged_in: false,
        }
    }

    /// create a new connection to gnudb.gnudb.org port 8880
    pub fn new() -> Self {
        Connection {
            stream: TcpStream::connect("gnudb.gnudb.org:8880").unwrap(),
            logged_in: false,
        }
    }

    /// login into gnudb
    pub fn login(&mut self) {
        println!("Successfully connected to server in port 8880");
        // say hello -> this is the login
        let mut hello = String::new();
        let mut reader = BufReader::new(self.stream.try_clone().unwrap());
        reader.read_line(&mut hello).unwrap();
        let hello = "cddb hello ripperx localhost ripperx 4\n".to_owned();
        send_command(&mut self.stream, hello).unwrap();

        // switch to protocol level 6, so the output of GNUDB contains DYEAR and DGENRE
        let proto = "proto 6\n".to_owned();
        send_command(&mut self.stream, proto).unwrap();
        self.logged_in = true;
    }

    /// search gnudb for a given discid
    pub fn search(&mut self, discid: &DiscId) -> Result<Disc, String> {
        if !self.logged_in {
            return Err("Not logged in".to_owned());
        }
        // the protocol is as follows:
        // cddb query discid ntrks off1 off2 ... nsecs

        // CDs don't *have* to start at track number 1...
        let count = discid.last_track_num() - discid.first_track_num() + 1;
        let mut toc = discid.toc_string();
        // the toc from DiscId is total_sectors first_track off1 off2 ... offn
        // so we take from the 3rd item in the toc
        toc = toc
            .match_indices(" ")
            .nth(2)
            .map(|(index, _)| toc.split_at(index))
            .unwrap()
            .1
            .to_owned();
        let query = format!(
            "cddb query {} {} {} {}\n",
            discid.freedb_id(),
            count,
            toc,
            discid.sectors() / 75
        );
        let disc = cddb_query(&mut self.stream, query, discid);

        return disc;
    }

    pub fn close(&mut self) {
        self.stream.shutdown(Shutdown::Both).unwrap();
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
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

/// send a CDDBP command, and parse its output, according to the protocol specs:
/// Server response code (three digit code):
///
/// First digit:
/// 1xx	Informative message
/// 2xx	Command OK
/// 3xx	Command OK so far, continue
/// 4xx	Command OK, but cannot be performed for some specified reasons
/// 5xx	Command unimplemented, incorrect, or program error
///
/// Second digit:
/// x0x	Ready for further commands
/// x1x	More server-to-client output follows (until terminating marker)
/// x2x	More client-to-server input follows (until terminating marker)
/// x3x	Connection will close
///
/// Third digit:
/// xx[0-9]	Command-specific code
fn send_command(stream: &mut TcpStream, cmd: String) -> Result<String, String> {
    let msg = cmd.as_bytes();
    stream.write(msg).unwrap();
    println!("sent {}", cmd);
    let mut response = String::new();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    match reader.read_line(&mut response) {
        Ok(_) => {
            println!("response: {}", response);
            if response.starts_with("5") {
                // kapoet
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
                        let result = reader.read_line(&mut response);
                        println!("response: {}", response);
                        match result {
                            Ok(_) => {
                                if response.starts_with(".") {
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

/// specific command to query the disc, first issues a query, and then a read
/// query protocol: cddb query discid ntrks off1 off2 ... nsecs
fn cddb_query(stream: &mut TcpStream, cmd: String, discid: &DiscId) -> Result<Disc, String> {
    let response = send_command(stream, cmd);
    if response.is_ok() {
        let response = response.unwrap();
        if response.starts_with("200") {
            // exact match
            let category = response.split(" ").nth(1).unwrap();
            let mut disc = cddb_read(category, discid.freedb_id().as_str(), stream);
            if disc.genre.is_none() {
                disc.genre = Some(category.to_owned());
            }
            stream.shutdown(Shutdown::Both).unwrap();
            return Ok(disc);
        } else if response.starts_with("211") {
            // inexact match - we just take first hit for now
            let response = response.lines().nth(1).unwrap();
            let mut split = response.split(" ");
            let category = split.next().unwrap();
            let discid = split.next().unwrap();
            let mut disc = cddb_read(category, discid, stream);
            if disc.genre.is_none() {
                disc.genre = Some(category.to_owned());
            }
            stream.shutdown(Shutdown::Both).unwrap();
            return Ok(disc);
        } else {
            stream.shutdown(Shutdown::Both).unwrap();
            return Err("failed to query disc".to_owned());
        }
    } else {
        return Err(response.err().unwrap());
    }
}

/// read and parse the disc info
/// protocol: cddb read category discid
fn cddb_read(category: &str, discid: &str, stream: &mut TcpStream) -> Disc {
    let get = format!("cddb read {} {}\n", category, discid);
    let data = send_command(stream, get).unwrap();
    let disc = parse_data(data);
    println!("disc:{:?}", disc);
    disc
}

/// parse the full response from the CDDB server
fn parse_data(data: String) -> Disc {
    println!("{}", data);
    let mut disc = Disc {
        ..Default::default()
    };
    let mut i = 0;
    for ref line in data.lines() {
        if line.starts_with("DTITLE") {
            let value = line.split("=").nth(1).unwrap();
            let mut split = value.split("/");
            disc.artist = split.next().unwrap().trim().to_owned();
            disc.title = split.next().unwrap().trim().to_owned();
        }
        if line.starts_with("DYEAR") {
            let value = line.split("=").nth(1).unwrap();
            disc.year = Some(value.parse::<u16>().unwrap());
        }
        if line.starts_with("DGENRE") {
            let value = line.split("=").nth(1).unwrap();
            disc.genre = Some(value.to_owned());
        }
        // since we use protocol level 6, we should get the year/genre via DYEAR and DGENRE, and these should come before EXTD
        // this is as a fallback
        if disc.year.is_none() && line.starts_with("EXTD") {
            // little bit awkward, can this be done better?
            let year_matches: Vec<_> = line.match_indices("YEAR:").collect();
            if year_matches.len() > 0 {
                let index = year_matches[0].0 + 6;
                let value = line.split_at(index).1;
                let space_matches: Vec<_> = value.match_indices(" ").collect();
                if space_matches.len() > 0 {
                    let value = value.split_at(space_matches[0].0).0;
                    disc.year = Some(value.parse::<u16>().unwrap());
                }
            }
        }
        if line.starts_with("TTITLE") {
            let mut track = Track {
                ..Default::default()
            };
            track.number = i + 1; // tracks are 0 based in CDDB/GNUDB
            track.title = line.split("=").nth(1).unwrap().to_owned();
            track.artist = disc.artist.clone();
            disc.tracks.push(track);
            i += 1; // assume tracks are consecutive - this is not necessarily true
        }
    }
    disc
}

#[cfg(test)]
mod test {
    use discid::DiscId;

    use crate::Connection;

    #[test]
    fn test_search() {
        let offsets = [
            185700, 150, 18051, 42248, 57183, 75952, 89333, 114384, 142453, 163641,
        ];
        let discid = DiscId::put(1, &offsets).unwrap();
        let mut con = Connection::new();
        con.login();
        let disc = con.search(&discid);
        assert!(disc.is_ok());
        let disc = disc.unwrap();
        assert_eq!(disc.year.unwrap(), 1978 as u16);
        assert_eq!(disc.tracks.len(), 9);
        assert_eq!(disc.genre.unwrap(), "Rock");
        assert_eq!(disc.title, "Dire Straits");
    }

    #[test]
    fn test_search_should_be_logged_in() {
        let offsets = [
            185700, 150, 18051, 42248, 57183, 75952, 89333, 114384, 142453, 163641,
        ];
        let discid = DiscId::put(1, &offsets).unwrap();
        let mut con = Connection::new();
        // no login -> search should fail
        let disc = con.search(&discid);
        assert!(disc.is_err());
    }

    #[test]
    fn test_parse() {
        let input = r"# xmcd
#
# Track frame offsets:
#	150
#	25075
#	46501
#	70596
#	88533
#	105910
#	125169
#	147365
#	162906
#	190441
#	215174
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
        assert_eq!(disc.year.unwrap(), 2002 as u16);
        assert_eq!(disc.title, "(black) Mutter");
        assert_eq!(disc.tracks.len(), 11);
        assert_eq!(disc.genre.unwrap(), "Industrial Metal");
    }

    #[test]
    fn test_extd() {
        let input = r"# xmcd
#
# Track frame offsets:
#	150
#	18051
#	42248
#	57183
#	75952
#	89333
#	114384
#	142453
#	163641
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
        assert_eq!(disc.year.unwrap(), 1978 as u16);
        assert_eq!(disc.genre.unwrap(), "Rock");
        assert_eq!(disc.tracks.len(), 9);
        assert_eq!(disc.title, "Dire Straits");
    }
}
